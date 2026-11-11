//! Wire snapshots of the orchestration engine (docs/20-refactor/libs.md,
//! card L3). A fixed script of calls with fixed times runs against a fresh
//! state dir; what lands in `run.json`, `mail.jsonl` and `index.json`, what
//! the public calls return and the errors they give are compared with JSON
//! frozen before `engine.rs` was split into modules.
//!
//! Only object key order is normalised (both sides are parsed values). Array
//! order, missing versus `null` fields, enum strings, error kinds and error
//! messages are compared as they are. A mismatch writes the actual JSON next
//! to the test binary (`CARGO_TARGET_TMPDIR`) and names the first differing
//! path; do not regenerate the golden to make a refactor pass.

use std::path::{Path, PathBuf};

use pod_orch::text;
use pod_orch::*;
use serde::Serialize;
use serde_json::{Map, Value, json};

fn temp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pod-orch-wire-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

/// A state dir removed when the test ends.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn to(v: &impl Serialize) -> Value {
    serde_json::to_value(v).unwrap()
}

fn err(e: OrchError) -> Value {
    json!({ "kind": format!("{:?}", e.kind), "message": e.message })
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn read_jsonl(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap();
    Value::Array(
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

fn states(v: &[(String, ItemState)]) -> Value {
    Value::Array(v.iter().map(|(i, s)| json!([i, s])).collect())
}

fn begin(
    o: &mut Orch,
    item: &str,
    token: &str,
    retry_of: Option<&str>,
    now: Ms,
) -> Result<Attempt, OrchError> {
    o.begin_attempt(
        BeginAttempt {
            run: "r1".into(),
            item: item.into(),
            agent: "codex".into(),
            place: "new".into(),
            retry_of: retry_of.map(str::to_string),
            token_hash: hash_token(token),
        },
        now,
    )
}

/// The first path where `actual` differs from `want`.
fn first_diff(path: &str, want: &Value, actual: &Value) -> Option<String> {
    match (want, actual) {
        (Value::Object(w), Value::Object(a)) => {
            for k in w.keys().chain(a.keys()) {
                let p = format!("{path}.{k}");
                match (w.get(k), a.get(k)) {
                    (Some(x), Some(y)) => {
                        if let Some(d) = first_diff(&p, x, y) {
                            return Some(d);
                        }
                    }
                    (x, y) => return Some(format!("{p}: want {x:?}, got {y:?}")),
                }
            }
            None
        }
        (Value::Array(w), Value::Array(a)) => {
            for (i, (x, y)) in w.iter().zip(a).enumerate() {
                if let Some(d) = first_diff(&format!("{path}[{i}]"), x, y) {
                    return Some(d);
                }
            }
            (w.len() != a.len())
                .then(|| format!("{path}: want {} elements, got {}", w.len(), a.len()))
        }
        _ => (want != actual).then(|| format!("{path}: want {want}, got {actual}")),
    }
}

fn check(name: &str, golden: &str, actual: Value) {
    let want: Value = serde_json::from_str(golden).unwrap();
    if want == actual {
        return;
    }
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.json"));
    std::fs::write(&out, serde_json::to_string_pretty(&actual).unwrap()).unwrap();
    panic!(
        "{name} differs from the golden at {}; actual JSON: {}",
        first_diff("$", &want, &actual).unwrap_or_default(),
        out.display()
    );
}

/// The fixed script: one run with items, dependencies, a gate, a queued
/// attempt, a retry, mail with a reply thread, group mail, a heartbeat,
/// wake-ups, read receipts, an accepted done, a repeated done, a reopen by
/// a handoff, a late done of a superseded attempt, a wait, a start failure,
/// an ended attempt, worker release and a remembered request. Returns what
/// the calls gave back and the errors they raised.
fn script(root: &Path) -> (Value, Value) {
    let mut o = Orch::load(root, Limits::default(), 1);
    let mut r = Map::new();
    let mut e = Map::new();

    e.insert(
        "create_run_empty".into(),
        err(o.create_run("  ", None, None, 2).unwrap_err()),
    );
    r.insert(
        "create_run".into(),
        to(&o
            .create_run("  ship the parser  ", Some("term_c"), None, 5)
            .unwrap()),
    );
    let i1 = o
        .add_item(
            "r1",
            None,
            "\n# Parse the header\n\nmore detail",
            &[],
            None,
            6,
        )
        .unwrap();
    let i2 = o
        .add_item(
            "r1",
            Some("  wire it  "),
            "connect",
            &["i1".into()],
            None,
            7,
        )
        .unwrap();
    let i3 = o
        .add_item("r1", Some("child"), "a sub task", &[], Some("i1"), 8)
        .unwrap();
    r.insert("add_item".into(), json!([to(&i1), to(&i2), to(&i3)]));
    e.insert(
        "add_item_empty_spec".into(),
        err(o.add_item("r1", None, "  ", &[], None, 9).unwrap_err()),
    );
    e.insert(
        "add_item_missing_dep".into(),
        err(o
            .add_item("r1", None, "x", &["i9".into()], None, 9)
            .unwrap_err()),
    );
    e.insert(
        "add_item_missing_run".into(),
        err(o.add_item("r9", None, "x", &[], None, 9).unwrap_err()),
    );
    e.insert(
        "begin_pending".into(),
        err(begin(&mut o, "i2", "k", None, 9).unwrap_err()),
    );

    let a1 = begin(&mut o, "i1", "k1", None, 10).unwrap();
    r.insert("begin_a1".into(), to(&a1.public()));
    r.insert(
        "attach_a1".into(),
        to(&o
            .attach_worker("a1", Some("task_1"), "term_w1", Some("brief-a1.md"), 3)
            .unwrap()
            .public()),
    );
    r.insert(
        "mark_ready_a1".into(),
        json!(o.mark_ready("a1", 12).unwrap()),
    );
    r.insert(
        "mark_ready_a1_again".into(),
        json!(o.mark_ready("a1", 12).unwrap()),
    );
    e.insert(
        "verify_wrong_token".into(),
        err(o.verify_token("a1", Some("wrong")).unwrap_err()),
    );
    e.insert(
        "verify_no_token".into(),
        err(o.verify_token("a1", None).unwrap_err()),
    );
    e.insert(
        "begin_assigned".into(),
        err(begin(&mut o, "i1", "k", None, 12).unwrap_err()),
    );
    e.insert(
        "begin_missing_item".into(),
        err(begin(&mut o, "i9", "k", None, 12).unwrap_err()),
    );

    let _a2 = begin(&mut o, "i3", "k2", None, 13).unwrap();
    r.insert(
        "enqueue_a2".into(),
        to(&o
            .enqueue(
                "a2",
                json!({"agent": "claude", "cwd": "w", "env": null}),
                Some("disk-w".into()),
                13,
            )
            .unwrap()),
    );
    r.insert(
        "queue".into(),
        json!(o.queue().iter().map(|a| a.id.clone()).collect::<Vec<_>>()),
    );
    r.insert("active_workers".into(), json!(o.active_workers()));
    r.insert(
        "block_i3".into(),
        to(&o.block_item("i3", "g1", 14).unwrap()),
    );
    r.insert(
        "block_i3_again".into(),
        to(&o.block_item("i3", "g1", 14).unwrap()),
    );
    e.insert(
        "begin_blocked".into(),
        err(begin(&mut o, "i3", "k", None, 14).unwrap_err()),
    );

    // Mail: a question, its answer (thread), group status, a heartbeat.
    let q = o
        .send(
            Some("r1"),
            NewMail::new("attempt:a1", "run:r1", MailType::Question, "which format?")
                .body("json or toml"),
            |_| false,
            15,
        )
        .unwrap();
    let mut ans = NewMail::new("run:r1", "attempt:a1", MailType::Answer, "json").body("use json");
    ans.reply_to = Some(q.id.clone());
    ans.priority = Priority::High;
    let a = o.send(Some("r1"), ans, |_| false, 16).unwrap();
    let group = o
        .send(
            Some("r1"),
            NewMail::pod(
                "run:r1",
                "@all",
                MailType::Status,
                MsgText::new(text::GROUP_CHECKED).with("n", 2),
            )
            .body_text(MsgText::new(text::NEXT_WORKING))
            .payload(json!({"note": null, "list": [3, 1, 2]})),
            |_| false,
            17,
        )
        .unwrap();
    let idle = o
        .send(
            Some("r1"),
            NewMail::new("run:r1", "@idle", MailType::Status, "idle only"),
            |a| a.id == "a2",
            17,
        )
        .unwrap();
    let beat = o
        .send(
            None,
            NewMail::new("attempt:a1", "run:r1", MailType::Heartbeat, "alive"),
            |_| false,
            17,
        )
        .unwrap();
    r.insert(
        "send".into(),
        json!([to(&q), to(&a), to(&group), to(&idle), to(&beat)]),
    );
    e.insert(
        "send_bad_address".into(),
        err(o
            .send(
                Some("r1"),
                NewMail::new("run:r1", "bogus", MailType::Status, "s"),
                |_| false,
                17,
            )
            .unwrap_err()),
    );
    e.insert(
        "send_done_to_group".into(),
        err(o
            .send(
                Some("r1"),
                NewMail::new("run:r1", "@all", MailType::Done, "s"),
                |_| false,
                17,
            )
            .unwrap_err()),
    );
    e.insert(
        "send_group_without_run".into(),
        err(o
            .send(
                None,
                NewMail::new("run:r1", "@codex", MailType::Status, "s"),
                |_| false,
                17,
            )
            .unwrap_err()),
    );
    e.insert(
        "send_empty_subject".into(),
        err(o
            .send(
                None,
                NewMail::new("run:r1", "attempt:a1", MailType::Status, " "),
                |_| false,
                17,
            )
            .unwrap_err()),
    );
    e.insert(
        "send_missing_attempt".into(),
        err(o
            .send(
                None,
                NewMail::new("run:r1", "attempt:a9", MailType::Status, "s"),
                |_| false,
                17,
            )
            .unwrap_err()),
    );

    // Wake-ups: scheduled, delivered for the run, failed for the worker.
    let both = ["run:r1".to_string(), "attempt:a1".to_string()];
    let wake = |o: &Orch| {
        json!({
            "run_pending": o.wake_pending("run:r1"),
            "run_notified": o.notified("run:r1"),
            "run_wants": o.wants_wake("run:r1"),
            "run_unread_waking": o.unread_waking("run:r1"),
            "a1_pending": o.wake_pending("attempt:a1"),
            "a1_unconfirmed": o.wake_unconfirmed("attempt:a1"),
            "a1_wants": o.wants_wake("attempt:a1"),
        })
    };
    let before = wake(&o);
    o.wake_scheduled(&both, 18).unwrap();
    let scheduled = wake(&o);
    o.wake_delivered(&both[..1], 19).unwrap();
    o.wake_failed(&both[1..], true).unwrap();
    let settled = wake(&o);
    r.insert(
        "wake".into(),
        json!({"before": before, "scheduled": scheduled, "settled": settled}),
    );
    r.insert(
        "mailbox_run".into(),
        json!([
            o.mailbox_run("run:r1"),
            o.mailbox_run("attempt:a2"),
            o.mailbox_run("run:r9"),
            o.mailbox_run("@all"),
        ]),
    );
    r.insert("unread_run".into(), to(&o.unread("run:r1")));
    r.insert(
        "check_a1".into(),
        to(&o.check(&both[1..], true, 20).unwrap()),
    );
    r.insert("wake_after_check".into(), wake(&o));
    r.insert(
        "check_a1_again".into(),
        to(&o.check(&both[1..], false, 21).unwrap()),
    );
    o.ack("run:r1", &["m1".into(), "m9".into()], 22).unwrap();
    r.insert("answer_to_m1".into(), to(&o.answer_to("m1")));
    r.insert(
        "has_open_question_a1".into(),
        json!(o.has_open_question("a1")),
    );

    // The accepted done, the same report again, a different report.
    let d = o
        .done(
            "a1",
            Some("k1"),
            true,
            "parsed",
            &["src/parse.rs".into()],
            Some("report.md"),
            Some(1000),
            30,
        )
        .unwrap();
    let done = |d: &DoneResult| {
        json!({
            "accepted": d.accepted,
            "repeated": d.repeated,
            "attempt": to(&d.attempt.public()),
            "mail": to(&d.mail),
        })
    };
    r.insert("done_a1".into(), done(&d));
    let again = o
        .done(
            "a1",
            Some("k1"),
            true,
            "parsed",
            &["src/parse.rs".into()],
            Some("report.md"),
            Some(1000),
            31,
        )
        .unwrap();
    r.insert("done_a1_repeated".into(), done(&again));
    e.insert(
        "done_a1_different".into(),
        err(o
            .done("a1", Some("k1"), false, "other", &[], None, None, 31)
            .unwrap_err()),
    );
    r.insert(
        "items_after_done".into(),
        to(&o.run("r1").unwrap().rec.items),
    );

    // A handoff reopens the finished worker; its second done closes the
    // new opening.
    let handoff = o
        .send(
            Some("r1"),
            NewMail::new("run:r1", "attempt:a1", MailType::Handoff, "one more thing"),
            |_| false,
            32,
        )
        .unwrap();
    r.insert("handoff".into(), to(&handoff));
    r.insert("a1_reopened".into(), to(&o.attempt("a1").unwrap().public()));
    r.insert(
        "done_a1_reopened".into(),
        done(
            &o.done("a1", Some("k1"), true, "also done", &[], None, None, 33)
                .unwrap(),
        ),
    );

    // A retry supersedes the queued attempt; its done is late.
    r.insert(
        "retry_a3".into(),
        to(&begin(&mut o, "i3", "k3", Some("a2"), 34).unwrap().public()),
    );
    e.insert(
        "retry_wrong_item".into(),
        err(begin(&mut o, "i2", "k", Some("a1"), 34).unwrap_err()),
    );
    r.insert(
        "done_a2_late".into(),
        done(
            &o.done("a2", Some("k2"), false, "too late", &[], None, None, 35)
                .unwrap(),
        ),
    );

    // Waiting for other items.
    r.insert(
        "wait_a3".into(),
        states(
            &o.wait_for_items("a3", &["i2".into(), "i1".into(), "i2".into()])
                .unwrap(),
        ),
    );
    e.insert(
        "wait_own_item".into(),
        err(o.wait_for_items("a3", &["i3".into()]).unwrap_err()),
    );
    e.insert(
        "wait_nothing".into(),
        err(o.wait_for_items("a3", &[]).unwrap_err()),
    );
    e.insert(
        "wait_closed_worker".into(),
        err(o.wait_for_items("a2", &["i1".into()]).unwrap_err()),
    );
    r.insert(
        "settle_waits_open".into(),
        json!(o.settle_waits().unwrap().len()),
    );
    r.insert(
        "unblock_i3".into(),
        to(&o.unblock_item("i3", "g1", 36).unwrap()),
    );
    r.insert(
        "unblock_i3_again".into(),
        to(&o.unblock_item("i3", "g1", 36).unwrap()),
    );

    // A start failure, an ended attempt, a reopened item.
    let i4 = o.add_item("r1", None, "fourth", &[], None, 37).unwrap();
    begin(&mut o, &i4.id, "k4", None, 37).unwrap();
    r.insert(
        "start_failed_a4".into(),
        json!(o.start_failed("a4", "spawn failed", 38).unwrap()),
    );
    begin(&mut o, &i4.id, "k5", None, 39).unwrap();
    r.insert(
        "end_a5".into(),
        json!(
            o.end_attempt(
                "a5",
                AttemptState::Lost,
                "terminal gone",
                None,
                Some(500),
                40
            )
            .unwrap()
        ),
    );
    r.insert(
        "end_a5_again".into(),
        json!(
            o.end_attempt("a5", AttemptState::Stopped, "stop", None, None, 41)
                .unwrap()
        ),
    );
    e.insert(
        "block_completed".into(),
        err(o.block_item("i1", "g2", 42).unwrap_err()),
    );
    r.insert("reopen_i1".into(), to(&o.reopen_item("i1", 42).unwrap()));
    begin(&mut o, "i2", "k6", None, 42).unwrap();
    r.insert(
        "done_a6_failed".into(),
        done(
            &o.done("a6", Some("k6"), false, "could not", &[], None, None, 42)
                .unwrap(),
        ),
    );
    o.update_attempt("a3", |a| {
        a.seen_turn = 7;
        a.escalated_turn = Some(6);
    })
    .unwrap();
    r.insert(
        "settle_waits".into(),
        json!(
            o.settle_waits()
                .unwrap()
                .iter()
                .map(|(a, s)| json!([a.id, states(s)]))
                .collect::<Vec<_>>()
        ),
    );

    // Worker release and lookups.
    r.insert(
        "due_releases".into(),
        json!([o.due_releases(39), o.due_releases(2000)]),
    );
    r.insert(
        "set_worker_kept".into(),
        to(&o
            .set_worker("a1", WorkerState::Kept, None, 43)
            .unwrap()
            .public()),
    );
    r.insert(
        "terminal_mailboxes".into(),
        json!([
            o.terminal_mailboxes("term_c"),
            o.terminal_mailboxes("term_w1"),
            o.runs_of_terminal("term_c"),
            o.mailbox_terminal("run:r1"),
            o.mailbox_terminal("attempt:a1"),
        ]),
    );
    r.insert(
        "set_worker_released".into(),
        to(&o
            .set_worker("a1", WorkerState::Released, Some("logs/a1.txt"), 44)
            .unwrap()
            .public()),
    );
    r.insert(
        "mailbox_terminal_released".into(),
        json!(o.mailbox_terminal("attempt:a1")),
    );
    e.insert(
        "send_to_released".into(),
        err(o
            .send(
                Some("r1"),
                NewMail::new("run:r1", "attempt:a1", MailType::Question, "still there?"),
                |_| false,
                45,
            )
            .unwrap_err()),
    );

    // Request ids.
    o.remember("req-1", "orch.item.add", json!({"item": "i1"}), 46)
        .unwrap();
    o.remember("req-2", "orch.send", json!(null), 47).unwrap();
    o.remember("req-1", "orch.item.add", json!({"item": "i4"}), 48)
        .unwrap();
    r.insert(
        "request".into(),
        json!([
            o.request("req-1", "orch.item.add").unwrap(),
            o.request("req-3", "orch.item.add").unwrap(),
        ]),
    );
    e.insert(
        "request_other_method".into(),
        err(o.request("req-1", "orch.send").unwrap_err()),
    );
    e.insert(
        "lookups".into(),
        json!([
            err(o.run("r9").unwrap_err()),
            err(o.item("i9").unwrap_err()),
            err(o.attempt("a9").unwrap_err()),
        ]),
    );
    // A delivered wake is persisted with the run; a failed one is not.
    o.wake_scheduled(&["run:r1".into(), "attempt:a3".into()], 49)
        .unwrap();
    o.wake_delivered(&["run:r1".into()], 50).unwrap();
    o.wake_failed(&["attempt:a3".into()], true).unwrap();
    r.insert(
        "wake_end".into(),
        json!([
            o.notified("run:r1"),
            o.wake_unconfirmed("attempt:a3"),
            o.wants_wake("attempt:a3"),
        ]),
    );
    r.insert("mail_of".into(), to(&o.mail_of("r1").unwrap()));
    r.insert("notes".into(), json!(o.notes));
    (Value::Object(r), Value::Object(e))
}

#[test]
fn records_after_a_fixed_script() {
    let dir = Dir(temp("records"));
    script(&dir.0);
    let root = &dir.0;
    let actual = json!({
        "run_json": read_json(&store::run_json(root, "r1")),
        "mail_jsonl": read_jsonl(&store::mail_jsonl(root, "r1")),
        "index_json": read_json(&store::index_json(root)),
    });
    check("records", RECORDS, actual);
}

#[test]
fn results_and_errors_of_a_fixed_script() {
    let dir = Dir(temp("results"));
    let (results, errors) = script(&dir.0);
    check(
        "results",
        RESULTS,
        json!({"results": results, "errors": errors}),
    );
}

/// After a restart: delivered wake-ups are kept, scheduled and unconfirmed
/// ones are not, read receipts and ids survive.
#[test]
fn reload_after_a_fixed_script() {
    let dir = Dir(temp("reload"));
    script(&dir.0);
    let mut o = Orch::load(&dir.0, Limits::default(), 100);
    let actual = json!({
        "notes": o.notes,
        "run": to(&o.run("r1").unwrap().rec),
        "mail_of": to(&o.mail_of("r1").unwrap()),
        "wake": {
            "run_notified": o.notified("run:r1"),
            "a3_unconfirmed": o.wake_unconfirmed("attempt:a3"),
            "a3_pending": o.wake_pending("attempt:a3"),
            "run_wants": o.wants_wake("run:r1"),
        },
        "next_ids": [
            to(&o.create_run("next", None, None, 101).unwrap()),
            to(&o.add_item("r2", None, "next item", &[], None, 102).unwrap()),
        ],
    });
    check("reload", RELOAD, actual);
}

/// Records written by an older daemon (every optional field missing) load
/// with their defaults; an accepted done that reached the mail but not the
/// run record is applied on load; ids continue after the highest one found.
#[test]
fn defaults_and_replay_on_load() {
    let dir = Dir(temp("defaults"));
    let root = &dir.0;
    std::fs::create_dir_all(store::run_dir(root, "r7")).unwrap();
    let run = json!({
        "version": 1,
        "run": {"id": "r7", "objective": "old", "created_ms": 1},
        "items": [
            {"id": "i5", "run": "r7", "title": "t", "spec": "s", "state": "assigned",
             "generation": 1, "attempt": "a4", "created_ms": 2, "updated_ms": 2},
            {"id": "i6", "run": "r7", "title": "u", "spec": "s", "deps": ["i5"],
             "state": "pending", "created_ms": 2, "updated_ms": 2}
        ],
        "attempts": [
            {"id": "a4", "run": "r7", "item": "i5", "generation": 1, "state": "ready",
             "worker": "active", "agent": "codex", "place": "new", "depth": 1,
             "created_ms": 3}
        ]
    });
    std::fs::write(store::run_json(root, "r7"), run.to_string()).unwrap();
    let lines = [
        json!({"kind": "mail", "mail": {"id": "m3", "run": "r7", "from": "run:r7",
            "to": "attempt:a4", "recipients": ["attempt:a4"], "type": "status",
            "subject": "s", "created_ms": 4}}),
        json!({"kind": "read", "mailbox": "attempt:a4", "ids": ["m3"], "ms": 5}),
        json!({"kind": "mail", "mail": {"id": "m4", "run": "r7", "from": "attempt:a4",
            "to": "run:r7", "recipients": ["run:r7"], "type": "done",
            "subject": "done", "body": "finished", "created_ms": 6,
            "payload": {"outcome": "succeeded", "item": "i5", "attempt": "a4",
                "files": ["a.rs"], "report": null, "accepted": true,
                "generation": 1, "reopens": 0, "release_at_ms": 900}}}),
    ];
    let jsonl: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(store::mail_jsonl(root, "r7"), jsonl).unwrap();

    let mut o = Orch::load(root, Limits::default(), 50);
    let loaded = json!({
        "notes": o.notes,
        "run": to(&o.run("r7").unwrap().rec),
        "run_json": read_json(&store::run_json(root, "r7")),
        "mail_of": to(&o.mail_of("r7").unwrap()),
        "unread_a4": to(&o.unread("attempt:a4")),
    });
    let next = json!([
        to(&o.create_run("new", None, None, 51).unwrap()),
        to(&o.add_item("r7", None, "more", &[], None, 52).unwrap()),
        to(&o
            .send(
                Some("r7"),
                NewMail::new("run:r7", "run:r7", MailType::Status, "n"),
                |_| false,
                53
            )
            .unwrap()),
    ]);
    let samples: Vec<Value> = [text::DONE, text::DONE_LATE, text::NEXT_WORKING]
        .iter()
        .map(|k| {
            let t = text::sample(k);
            json!({"text": to(&t), "en": t.en()})
        })
        .collect();
    check(
        "defaults",
        DEFAULTS,
        json!({"loaded": loaded, "next": next, "msg_text": samples}),
    );
}

const RECORDS: &str = r##"{
  "index_json": {
    "counters": {
      "a": 6,
      "i": 4,
      "m": 10,
      "r": 1
    },
    "requests": [
      {
        "id": "req-2",
        "method": "orch.send",
        "ms": 47,
        "result": null
      },
      {
        "id": "req-1",
        "method": "orch.item.add",
        "ms": 48,
        "result": {
          "item": "i4"
        }
      }
    ]
  },
  "mail_jsonl": [
    {
      "kind": "mail",
      "mail": {
        "body": "json or toml",
        "created_ms": 15,
        "from": "attempt:a1",
        "id": "m1",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "which format?",
        "to": "run:r1",
        "type": "question"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "use json",
        "created_ms": 16,
        "from": "run:r1",
        "id": "m2",
        "priority": "high",
        "recipients": [
          "attempt:a1"
        ],
        "reply_to": "m1",
        "run": "r1",
        "subject": "json",
        "thread": "m1",
        "to": "attempt:a1",
        "type": "answer"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "working",
        "body_text": {
          "kind": "next.working"
        },
        "created_ms": 17,
        "from": "run:r1",
        "id": "m3",
        "payload": {
          "list": [
            3,
            1,
            2
          ],
          "note": null
        },
        "priority": "normal",
        "recipients": [
          "attempt:a1",
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "group  check  (no verdict)",
        "text": {
          "kind": "group.checked",
          "params": {
            "n": "2"
          }
        },
        "to": "@all",
        "type": "status"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "",
        "created_ms": 17,
        "from": "run:r1",
        "id": "m4",
        "priority": "normal",
        "recipients": [
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "idle only",
        "to": "@idle",
        "type": "status"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "",
        "created_ms": 17,
        "from": "attempt:a1",
        "id": "m5",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "alive",
        "to": "run:r1",
        "type": "heartbeat"
      }
    },
    {
      "ids": [
        "m2",
        "m3"
      ],
      "kind": "read",
      "mailbox": "attempt:a1",
      "ms": 20
    },
    {
      "ids": [
        "m1"
      ],
      "kind": "read",
      "mailbox": "run:r1",
      "ms": 22
    },
    {
      "kind": "mail",
      "mail": {
        "body": "parsed",
        "created_ms": 30,
        "from": "attempt:a1",
        "id": "m6",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [
            "src/parse.rs"
          ],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": 1030,
          "reopens": 0,
          "report": "report.md"
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "",
        "created_ms": 32,
        "from": "run:r1",
        "id": "m7",
        "priority": "normal",
        "recipients": [
          "attempt:a1"
        ],
        "run": "r1",
        "subject": "one more thing",
        "to": "attempt:a1",
        "type": "handoff"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "also done",
        "created_ms": 33,
        "from": "attempt:a1",
        "id": "m8",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": null,
          "reopens": 1,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "too late",
        "created_ms": 35,
        "fenced": true,
        "from": "attempt:a2",
        "id": "m9",
        "payload": {
          "accepted": false,
          "attempt": "a2",
          "files": [],
          "generation": 1,
          "item": "i3",
          "outcome": "failed",
          "release_at_ms": null,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "late done (failed) from superseded attempt a2; ignored",
        "text": {
          "kind": "done.late",
          "params": {
            "attempt": "a2",
            "item": "i3",
            "outcome": "failed"
          }
        },
        "to": "run:r1",
        "type": "done"
      }
    },
    {
      "kind": "mail",
      "mail": {
        "body": "could not",
        "created_ms": 42,
        "from": "attempt:a6",
        "id": "m10",
        "payload": {
          "accepted": true,
          "attempt": "a6",
          "files": [],
          "generation": 1,
          "item": "i2",
          "outcome": "failed",
          "release_at_ms": null,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (failed): wire it",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a6",
            "item": "i2",
            "outcome": "failed",
            "title": "wire it"
          }
        },
        "to": "run:r1",
        "type": "done"
      }
    }
  ],
  "run_json": {
    "attempts": [
      {
        "agent": "codex",
        "body": "also done",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "depth": 1,
        "ended_ms": 33,
        "generation": 1,
        "id": "a1",
        "item": "i1",
        "log_path": "logs/a1.txt",
        "place": "new",
        "ready_ms": 12,
        "released_ms": 44,
        "reopens": 1,
        "run": "r1",
        "seen_turn": 3,
        "state": "succeeded",
        "task": "task_1",
        "terminal": "term_w1",
        "token_hash": "6ab9f1eb8f7d3388f4f9d586f66e99fd54080df2c446f0e58668b09c08a16dd0",
        "worker": "released"
      },
      {
        "agent": "codex",
        "created_ms": 13,
        "depth": 1,
        "ended_ms": 34,
        "error": "replaced by a retry",
        "generation": 1,
        "id": "a2",
        "item": "i3",
        "place": "new",
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "abandoned",
        "token_hash": "015f7e6bc5aeaf483724089e9252cc13b50951a6b69412522765cff4d780306e",
        "worker": "kept"
      },
      {
        "agent": "codex",
        "created_ms": 34,
        "depth": 1,
        "escalated_turn": 6,
        "generation": 2,
        "id": "a3",
        "item": "i3",
        "place": "new",
        "reopens": 0,
        "retry_of": "a2",
        "run": "r1",
        "seen_turn": 7,
        "state": "starting",
        "token_hash": "2f5052c9fd15b19a18c584d01363568198613f0c34e84409ef7938709a159ec2",
        "worker": "active"
      },
      {
        "agent": "codex",
        "created_ms": 37,
        "depth": 1,
        "ended_ms": 38,
        "error": "spawn failed",
        "generation": 1,
        "id": "a4",
        "item": "i4",
        "place": "new",
        "released_ms": 38,
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "start_failed",
        "token_hash": "94091dd64a21ffe94214bc6d17deeb43873a5cf2f0a71b4b5caa9a5c81b6967d",
        "worker": "released"
      },
      {
        "agent": "codex",
        "created_ms": 39,
        "depth": 1,
        "ended_ms": 40,
        "error": "terminal gone",
        "generation": 2,
        "id": "a5",
        "item": "i4",
        "place": "new",
        "release_at_ms": 540,
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "lost",
        "token_hash": "88dbf612972c594a2c2f6dde493ef1c4cd27adba8c36e6ebd00b89ff143aba61",
        "worker": "finished"
      },
      {
        "agent": "codex",
        "body": "could not",
        "created_ms": 42,
        "depth": 1,
        "ended_ms": 42,
        "generation": 1,
        "id": "a6",
        "item": "i2",
        "place": "new",
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "failed",
        "token_hash": "1d92ad4b6987fa0347cc5d2fb6cf9e47c83f4f6caeb3b5ef6f629730528921c3",
        "worker": "finished"
      }
    ],
    "items": [
      {
        "attempt": "a1",
        "circuit_open": false,
        "created_ms": 6,
        "generation": 1,
        "id": "i1",
        "run": "r1",
        "spec": "\n# Parse the header\n\nmore detail",
        "start_failures": 0,
        "state": "ready",
        "title": "Parse the header",
        "updated_ms": 42
      },
      {
        "attempt": "a6",
        "circuit_open": false,
        "created_ms": 7,
        "deps": [
          "i1"
        ],
        "generation": 1,
        "id": "i2",
        "run": "r1",
        "spec": "connect",
        "start_failures": 0,
        "state": "failed",
        "title": "wire it",
        "updated_ms": 42
      },
      {
        "attempt": "a3",
        "circuit_open": false,
        "created_ms": 8,
        "generation": 2,
        "id": "i3",
        "parent": "i1",
        "run": "r1",
        "spec": "a sub task",
        "start_failures": 0,
        "state": "assigned",
        "title": "child",
        "updated_ms": 36
      },
      {
        "attempt": "a5",
        "circuit_open": false,
        "created_ms": 37,
        "generation": 2,
        "id": "i4",
        "run": "r1",
        "spec": "fourth",
        "start_failures": 1,
        "state": "ready",
        "title": "fourth",
        "updated_ms": 40
      }
    ],
    "run": {
      "coordinator_terminal": "term_c",
      "created_ms": 5,
      "depth": 0,
      "id": "r1",
      "objective": "ship the parser"
    },
    "version": 1,
    "wakes": {
      "attempt:a3": {},
      "run:r1": {
        "notified_ms": 50
      }
    }
  }
}"##;

const RESULTS: &str = r##"{
  "errors": {
    "add_item_empty_spec": {
      "kind": "InvalidParams",
      "message": "the spec must not be empty"
    },
    "add_item_missing_dep": {
      "kind": "ItemNotFound",
      "message": "no item i9 in run r1"
    },
    "add_item_missing_run": {
      "kind": "RunNotFound",
      "message": "no run r9"
    },
    "begin_assigned": {
      "kind": "InvalidState",
      "message": "item i1 already has attempt a1; pass --retry-of a1 to replace it"
    },
    "begin_blocked": {
      "kind": "InvalidState",
      "message": "item i3 waits for a decision (gate g1); decide it first"
    },
    "begin_missing_item": {
      "kind": "ItemNotFound",
      "message": "no item i9 in run r1"
    },
    "begin_pending": {
      "kind": "DepsNotDone",
      "message": "item i2 waits for its dependencies"
    },
    "block_completed": {
      "kind": "InvalidState",
      "message": "item i1 is already completed"
    },
    "create_run_empty": {
      "kind": "InvalidParams",
      "message": "the objective must not be empty"
    },
    "done_a1_different": {
      "kind": "InvalidState",
      "message": "attempt a1 already reported done; send a status message instead"
    },
    "lookups": [
      {
        "kind": "RunNotFound",
        "message": "no run r9"
      },
      {
        "kind": "ItemNotFound",
        "message": "no item i9"
      },
      {
        "kind": "AttemptNotFound",
        "message": "no attempt a9"
      }
    ],
    "request_other_method": {
      "kind": "InvalidParams",
      "message": "request_id req-1 was used for orch.item.add"
    },
    "retry_wrong_item": {
      "kind": "InvalidParams",
      "message": "attempt a1 belongs to item i1, not i2"
    },
    "send_bad_address": {
      "kind": "InvalidParams",
      "message": "bad address \"bogus\": use run:<id>, attempt:<id>, @all, @idle, @claude or @codex"
    },
    "send_done_to_group": {
      "kind": "InvalidParams",
      "message": "done cannot go to a group"
    },
    "send_empty_subject": {
      "kind": "InvalidParams",
      "message": "the subject must not be empty"
    },
    "send_group_without_run": {
      "kind": "InvalidParams",
      "message": "@codex needs a run (--run)"
    },
    "send_missing_attempt": {
      "kind": "AttemptNotFound",
      "message": "no attempt a9"
    },
    "send_to_released": {
      "kind": "InvalidState",
      "message": "worker a1 was released; start a new attempt (--retry-of a1)"
    },
    "verify_no_token": {
      "kind": "AttemptFenced",
      "message": "the token does not belong to attempt a1 (POD_ATTEMPT_TOKEN)"
    },
    "verify_wrong_token": {
      "kind": "AttemptFenced",
      "message": "the token does not belong to attempt a1 (POD_ATTEMPT_TOKEN)"
    },
    "wait_closed_worker": {
      "kind": "InvalidState",
      "message": "worker a2 is Abandoned, not running"
    },
    "wait_nothing": {
      "kind": "InvalidParams",
      "message": "name at least one item (--item)"
    },
    "wait_own_item": {
      "kind": "InvalidParams",
      "message": "i3 is this worker's own item"
    }
  },
  "results": {
    "a1_reopened": {
      "agent": "codex",
      "body": "parsed",
      "brief_path": "brief-a1.md",
      "created_ms": 10,
      "depth": 1,
      "files": [
        "src/parse.rs"
      ],
      "generation": 1,
      "id": "a1",
      "item": "i1",
      "place": "new",
      "ready_ms": 12,
      "reopens": 1,
      "report": "report.md",
      "run": "r1",
      "seen_turn": 3,
      "state": "ready",
      "task": "task_1",
      "terminal": "term_w1",
      "worker": "active"
    },
    "active_workers": 1,
    "add_item": [
      {
        "circuit_open": false,
        "created_ms": 6,
        "generation": 0,
        "id": "i1",
        "run": "r1",
        "spec": "\n# Parse the header\n\nmore detail",
        "start_failures": 0,
        "state": "ready",
        "title": "Parse the header",
        "updated_ms": 6
      },
      {
        "circuit_open": false,
        "created_ms": 7,
        "deps": [
          "i1"
        ],
        "generation": 0,
        "id": "i2",
        "run": "r1",
        "spec": "connect",
        "start_failures": 0,
        "state": "pending",
        "title": "wire it",
        "updated_ms": 7
      },
      {
        "circuit_open": false,
        "created_ms": 8,
        "generation": 0,
        "id": "i3",
        "parent": "i1",
        "run": "r1",
        "spec": "a sub task",
        "start_failures": 0,
        "state": "ready",
        "title": "child",
        "updated_ms": 8
      }
    ],
    "answer_to_m1": {
      "body": "use json",
      "created_ms": 16,
      "from": "run:r1",
      "id": "m2",
      "priority": "high",
      "recipients": [
        "attempt:a1"
      ],
      "reply_to": "m1",
      "run": "r1",
      "subject": "json",
      "thread": "m1",
      "to": "attempt:a1",
      "type": "answer"
    },
    "attach_a1": {
      "agent": "codex",
      "brief_path": "brief-a1.md",
      "created_ms": 10,
      "depth": 1,
      "generation": 1,
      "id": "a1",
      "item": "i1",
      "place": "new",
      "reopens": 0,
      "run": "r1",
      "seen_turn": 3,
      "state": "starting",
      "task": "task_1",
      "terminal": "term_w1",
      "worker": "active"
    },
    "begin_a1": {
      "agent": "codex",
      "created_ms": 10,
      "depth": 1,
      "generation": 1,
      "id": "a1",
      "item": "i1",
      "place": "new",
      "reopens": 0,
      "run": "r1",
      "seen_turn": 0,
      "state": "starting",
      "worker": "active"
    },
    "block_i3": {
      "attempt": "a2",
      "circuit_open": false,
      "created_ms": 8,
      "gates": [
        "g1"
      ],
      "generation": 1,
      "id": "i3",
      "parent": "i1",
      "run": "r1",
      "spec": "a sub task",
      "start_failures": 0,
      "state": "blocked",
      "title": "child",
      "updated_ms": 14
    },
    "block_i3_again": {
      "attempt": "a2",
      "circuit_open": false,
      "created_ms": 8,
      "gates": [
        "g1"
      ],
      "generation": 1,
      "id": "i3",
      "parent": "i1",
      "run": "r1",
      "spec": "a sub task",
      "start_failures": 0,
      "state": "blocked",
      "title": "child",
      "updated_ms": 14
    },
    "check_a1": [
      {
        "body": "use json",
        "created_ms": 16,
        "from": "run:r1",
        "id": "m2",
        "priority": "high",
        "read_ms": 20,
        "recipients": [
          "attempt:a1"
        ],
        "reply_to": "m1",
        "run": "r1",
        "subject": "json",
        "thread": "m1",
        "to": "attempt:a1",
        "type": "answer"
      },
      {
        "body": "working",
        "body_text": {
          "kind": "next.working"
        },
        "created_ms": 17,
        "from": "run:r1",
        "id": "m3",
        "payload": {
          "list": [
            3,
            1,
            2
          ],
          "note": null
        },
        "priority": "normal",
        "read_ms": 20,
        "recipients": [
          "attempt:a1",
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "group  check  (no verdict)",
        "text": {
          "kind": "group.checked",
          "params": {
            "n": "2"
          }
        },
        "to": "@all",
        "type": "status"
      }
    ],
    "check_a1_again": [],
    "create_run": {
      "coordinator_terminal": "term_c",
      "created_ms": 5,
      "depth": 0,
      "id": "r1",
      "objective": "ship the parser"
    },
    "done_a1": {
      "accepted": true,
      "attempt": {
        "agent": "codex",
        "body": "parsed",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "depth": 1,
        "ended_ms": 30,
        "files": [
          "src/parse.rs"
        ],
        "generation": 1,
        "id": "a1",
        "item": "i1",
        "place": "new",
        "ready_ms": 12,
        "release_at_ms": 1030,
        "reopens": 0,
        "report": "report.md",
        "run": "r1",
        "seen_turn": 3,
        "state": "succeeded",
        "task": "task_1",
        "terminal": "term_w1",
        "worker": "finished"
      },
      "mail": {
        "body": "parsed",
        "created_ms": 30,
        "from": "attempt:a1",
        "id": "m6",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [
            "src/parse.rs"
          ],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": 1030,
          "reopens": 0,
          "report": "report.md"
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      "repeated": false
    },
    "done_a1_reopened": {
      "accepted": true,
      "attempt": {
        "agent": "codex",
        "body": "also done",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "depth": 1,
        "ended_ms": 33,
        "generation": 1,
        "id": "a1",
        "item": "i1",
        "place": "new",
        "ready_ms": 12,
        "reopens": 1,
        "run": "r1",
        "seen_turn": 3,
        "state": "succeeded",
        "task": "task_1",
        "terminal": "term_w1",
        "worker": "finished"
      },
      "mail": {
        "body": "also done",
        "created_ms": 33,
        "from": "attempt:a1",
        "id": "m8",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": null,
          "reopens": 1,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      "repeated": false
    },
    "done_a1_repeated": {
      "accepted": true,
      "attempt": {
        "agent": "codex",
        "body": "parsed",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "depth": 1,
        "ended_ms": 30,
        "files": [
          "src/parse.rs"
        ],
        "generation": 1,
        "id": "a1",
        "item": "i1",
        "place": "new",
        "ready_ms": 12,
        "release_at_ms": 1030,
        "reopens": 0,
        "report": "report.md",
        "run": "r1",
        "seen_turn": 3,
        "state": "succeeded",
        "task": "task_1",
        "terminal": "term_w1",
        "worker": "finished"
      },
      "mail": {
        "body": "parsed",
        "created_ms": 30,
        "from": "attempt:a1",
        "id": "m6",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [
            "src/parse.rs"
          ],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": 1030,
          "reopens": 0,
          "report": "report.md"
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      "repeated": true
    },
    "done_a2_late": {
      "accepted": false,
      "attempt": {
        "agent": "codex",
        "created_ms": 13,
        "depth": 1,
        "ended_ms": 34,
        "error": "replaced by a retry",
        "generation": 1,
        "id": "a2",
        "item": "i3",
        "place": "new",
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "abandoned",
        "worker": "kept"
      },
      "mail": {
        "body": "too late",
        "created_ms": 35,
        "fenced": true,
        "from": "attempt:a2",
        "id": "m9",
        "payload": {
          "accepted": false,
          "attempt": "a2",
          "files": [],
          "generation": 1,
          "item": "i3",
          "outcome": "failed",
          "release_at_ms": null,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "late done (failed) from superseded attempt a2; ignored",
        "text": {
          "kind": "done.late",
          "params": {
            "attempt": "a2",
            "item": "i3",
            "outcome": "failed"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      "repeated": false
    },
    "done_a6_failed": {
      "accepted": true,
      "attempt": {
        "agent": "codex",
        "body": "could not",
        "created_ms": 42,
        "depth": 1,
        "ended_ms": 42,
        "generation": 1,
        "id": "a6",
        "item": "i2",
        "place": "new",
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "failed",
        "worker": "finished"
      },
      "mail": {
        "body": "could not",
        "created_ms": 42,
        "from": "attempt:a6",
        "id": "m10",
        "payload": {
          "accepted": true,
          "attempt": "a6",
          "files": [],
          "generation": 1,
          "item": "i2",
          "outcome": "failed",
          "release_at_ms": null,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (failed): wire it",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a6",
            "item": "i2",
            "outcome": "failed",
            "title": "wire it"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      "repeated": false
    },
    "due_releases": [
      [],
      [
        "a5"
      ]
    ],
    "end_a5": true,
    "end_a5_again": false,
    "enqueue_a2": {
      "agent": "codex",
      "created_ms": 13,
      "depth": 1,
      "generation": 1,
      "id": "a2",
      "item": "i3",
      "launch": {
        "agent": "claude",
        "cwd": "w",
        "env": null
      },
      "place": "new",
      "queue": {
        "disk_path": "disk-w",
        "kind": "queued",
        "position": 0,
        "reason": "자원 확인을 기다리는 중이에요.",
        "since_ms": 13
      },
      "reopens": 0,
      "run": "r1",
      "seen_turn": 0,
      "state": "starting",
      "token_hash": "015f7e6bc5aeaf483724089e9252cc13b50951a6b69412522765cff4d780306e",
      "worker": "active"
    },
    "handoff": {
      "body": "",
      "created_ms": 32,
      "from": "run:r1",
      "id": "m7",
      "priority": "normal",
      "recipients": [
        "attempt:a1"
      ],
      "run": "r1",
      "subject": "one more thing",
      "to": "attempt:a1",
      "type": "handoff"
    },
    "has_open_question_a1": false,
    "items_after_done": [
      {
        "attempt": "a1",
        "circuit_open": false,
        "created_ms": 6,
        "generation": 1,
        "id": "i1",
        "run": "r1",
        "spec": "\n# Parse the header\n\nmore detail",
        "start_failures": 0,
        "state": "completed",
        "title": "Parse the header",
        "updated_ms": 30
      },
      {
        "circuit_open": false,
        "created_ms": 7,
        "deps": [
          "i1"
        ],
        "generation": 0,
        "id": "i2",
        "run": "r1",
        "spec": "connect",
        "start_failures": 0,
        "state": "ready",
        "title": "wire it",
        "updated_ms": 30
      },
      {
        "attempt": "a2",
        "circuit_open": false,
        "created_ms": 8,
        "gates": [
          "g1"
        ],
        "generation": 1,
        "id": "i3",
        "parent": "i1",
        "run": "r1",
        "spec": "a sub task",
        "start_failures": 0,
        "state": "blocked",
        "title": "child",
        "updated_ms": 14
      }
    ],
    "mail_of": [
      {
        "body": "json or toml",
        "created_ms": 15,
        "from": "attempt:a1",
        "id": "m1",
        "priority": "normal",
        "read_ms": 22,
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "which format?",
        "to": "run:r1",
        "type": "question"
      },
      {
        "body": "use json",
        "created_ms": 16,
        "from": "run:r1",
        "id": "m2",
        "priority": "high",
        "read_ms": 20,
        "recipients": [
          "attempt:a1"
        ],
        "reply_to": "m1",
        "run": "r1",
        "subject": "json",
        "thread": "m1",
        "to": "attempt:a1",
        "type": "answer"
      },
      {
        "body": "working",
        "body_text": {
          "kind": "next.working"
        },
        "created_ms": 17,
        "from": "run:r1",
        "id": "m3",
        "payload": {
          "list": [
            3,
            1,
            2
          ],
          "note": null
        },
        "priority": "normal",
        "read_ms": 20,
        "recipients": [
          "attempt:a1",
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "group  check  (no verdict)",
        "text": {
          "kind": "group.checked",
          "params": {
            "n": "2"
          }
        },
        "to": "@all",
        "type": "status"
      },
      {
        "body": "",
        "created_ms": 17,
        "from": "run:r1",
        "id": "m4",
        "priority": "normal",
        "recipients": [
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "idle only",
        "to": "@idle",
        "type": "status"
      },
      {
        "body": "",
        "created_ms": 17,
        "from": "attempt:a1",
        "id": "m5",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "alive",
        "to": "run:r1",
        "type": "heartbeat"
      },
      {
        "body": "parsed",
        "created_ms": 30,
        "from": "attempt:a1",
        "id": "m6",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [
            "src/parse.rs"
          ],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": 1030,
          "reopens": 0,
          "report": "report.md"
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      {
        "body": "",
        "created_ms": 32,
        "from": "run:r1",
        "id": "m7",
        "priority": "normal",
        "recipients": [
          "attempt:a1"
        ],
        "run": "r1",
        "subject": "one more thing",
        "to": "attempt:a1",
        "type": "handoff"
      },
      {
        "body": "also done",
        "created_ms": 33,
        "from": "attempt:a1",
        "id": "m8",
        "payload": {
          "accepted": true,
          "attempt": "a1",
          "files": [],
          "generation": 1,
          "item": "i1",
          "outcome": "succeeded",
          "release_at_ms": null,
          "reopens": 1,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (succeeded): Parse the header",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      {
        "body": "too late",
        "created_ms": 35,
        "fenced": true,
        "from": "attempt:a2",
        "id": "m9",
        "payload": {
          "accepted": false,
          "attempt": "a2",
          "files": [],
          "generation": 1,
          "item": "i3",
          "outcome": "failed",
          "release_at_ms": null,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "late done (failed) from superseded attempt a2; ignored",
        "text": {
          "kind": "done.late",
          "params": {
            "attempt": "a2",
            "item": "i3",
            "outcome": "failed"
          }
        },
        "to": "run:r1",
        "type": "done"
      },
      {
        "body": "could not",
        "created_ms": 42,
        "from": "attempt:a6",
        "id": "m10",
        "payload": {
          "accepted": true,
          "attempt": "a6",
          "files": [],
          "generation": 1,
          "item": "i2",
          "outcome": "failed",
          "release_at_ms": null,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "done (failed): wire it",
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a6",
            "item": "i2",
            "outcome": "failed",
            "title": "wire it"
          }
        },
        "to": "run:r1",
        "type": "done"
      }
    ],
    "mailbox_run": [
      "r1",
      "r1",
      null,
      null
    ],
    "mailbox_terminal_released": null,
    "mark_ready_a1": true,
    "mark_ready_a1_again": false,
    "notes": [],
    "queue": [
      "a2"
    ],
    "reopen_i1": {
      "attempt": "a1",
      "circuit_open": false,
      "created_ms": 6,
      "generation": 1,
      "id": "i1",
      "run": "r1",
      "spec": "\n# Parse the header\n\nmore detail",
      "start_failures": 0,
      "state": "ready",
      "title": "Parse the header",
      "updated_ms": 42
    },
    "request": [
      {
        "item": "i4"
      },
      null
    ],
    "retry_a3": {
      "agent": "codex",
      "created_ms": 34,
      "depth": 1,
      "generation": 2,
      "id": "a3",
      "item": "i3",
      "place": "new",
      "reopens": 0,
      "retry_of": "a2",
      "run": "r1",
      "seen_turn": 0,
      "state": "starting",
      "worker": "active"
    },
    "send": [
      {
        "body": "json or toml",
        "created_ms": 15,
        "from": "attempt:a1",
        "id": "m1",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "which format?",
        "to": "run:r1",
        "type": "question"
      },
      {
        "body": "use json",
        "created_ms": 16,
        "from": "run:r1",
        "id": "m2",
        "priority": "high",
        "recipients": [
          "attempt:a1"
        ],
        "reply_to": "m1",
        "run": "r1",
        "subject": "json",
        "thread": "m1",
        "to": "attempt:a1",
        "type": "answer"
      },
      {
        "body": "working",
        "body_text": {
          "kind": "next.working"
        },
        "created_ms": 17,
        "from": "run:r1",
        "id": "m3",
        "payload": {
          "list": [
            3,
            1,
            2
          ],
          "note": null
        },
        "priority": "normal",
        "recipients": [
          "attempt:a1",
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "group  check  (no verdict)",
        "text": {
          "kind": "group.checked",
          "params": {
            "n": "2"
          }
        },
        "to": "@all",
        "type": "status"
      },
      {
        "body": "",
        "created_ms": 17,
        "from": "run:r1",
        "id": "m4",
        "priority": "normal",
        "recipients": [
          "attempt:a2"
        ],
        "run": "r1",
        "subject": "idle only",
        "to": "@idle",
        "type": "status"
      },
      {
        "body": "",
        "created_ms": 17,
        "from": "attempt:a1",
        "id": "m5",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "alive",
        "to": "run:r1",
        "type": "heartbeat"
      }
    ],
    "set_worker_kept": {
      "agent": "codex",
      "body": "also done",
      "brief_path": "brief-a1.md",
      "created_ms": 10,
      "depth": 1,
      "ended_ms": 33,
      "generation": 1,
      "id": "a1",
      "item": "i1",
      "place": "new",
      "ready_ms": 12,
      "reopens": 1,
      "run": "r1",
      "seen_turn": 3,
      "state": "succeeded",
      "task": "task_1",
      "terminal": "term_w1",
      "worker": "kept"
    },
    "set_worker_released": {
      "agent": "codex",
      "body": "also done",
      "brief_path": "brief-a1.md",
      "created_ms": 10,
      "depth": 1,
      "ended_ms": 33,
      "generation": 1,
      "id": "a1",
      "item": "i1",
      "log_path": "logs/a1.txt",
      "place": "new",
      "ready_ms": 12,
      "released_ms": 44,
      "reopens": 1,
      "run": "r1",
      "seen_turn": 3,
      "state": "succeeded",
      "task": "task_1",
      "terminal": "term_w1",
      "worker": "released"
    },
    "settle_waits": [
      [
        "a3",
        [
          [
            "i2",
            "failed"
          ]
        ]
      ]
    ],
    "settle_waits_open": 0,
    "start_failed_a4": false,
    "terminal_mailboxes": [
      [
        "run:r1"
      ],
      [
        "attempt:a1"
      ],
      [
        "r1"
      ],
      "term_c",
      "term_w1"
    ],
    "unblock_i3": {
      "attempt": "a3",
      "circuit_open": false,
      "created_ms": 8,
      "generation": 2,
      "id": "i3",
      "parent": "i1",
      "run": "r1",
      "spec": "a sub task",
      "start_failures": 0,
      "state": "assigned",
      "title": "child",
      "updated_ms": 36
    },
    "unblock_i3_again": null,
    "unread_run": [
      {
        "body": "json or toml",
        "created_ms": 15,
        "from": "attempt:a1",
        "id": "m1",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "which format?",
        "to": "run:r1",
        "type": "question"
      },
      {
        "body": "",
        "created_ms": 17,
        "from": "attempt:a1",
        "id": "m5",
        "priority": "normal",
        "recipients": [
          "run:r1"
        ],
        "run": "r1",
        "subject": "alive",
        "to": "run:r1",
        "type": "heartbeat"
      }
    ],
    "wait_a3": [
      [
        "i2",
        "ready"
      ],
      [
        "i1",
        "completed"
      ]
    ],
    "wake": {
      "before": {
        "a1_pending": false,
        "a1_unconfirmed": 0,
        "a1_wants": true,
        "run_notified": null,
        "run_pending": false,
        "run_unread_waking": 1,
        "run_wants": true
      },
      "scheduled": {
        "a1_pending": true,
        "a1_unconfirmed": 0,
        "a1_wants": false,
        "run_notified": null,
        "run_pending": true,
        "run_unread_waking": 1,
        "run_wants": false
      },
      "settled": {
        "a1_pending": false,
        "a1_unconfirmed": 1,
        "a1_wants": false,
        "run_notified": 19,
        "run_pending": false,
        "run_unread_waking": 1,
        "run_wants": false
      }
    },
    "wake_after_check": {
      "a1_pending": false,
      "a1_unconfirmed": 0,
      "a1_wants": false,
      "run_notified": 19,
      "run_pending": false,
      "run_unread_waking": 1,
      "run_wants": false
    },
    "wake_end": [
      50,
      1,
      false
    ]
  }
}"##;

const RELOAD: &str = r##"{
  "mail_of": [
    {
      "body": "json or toml",
      "created_ms": 15,
      "from": "attempt:a1",
      "id": "m1",
      "priority": "normal",
      "read_ms": 22,
      "recipients": [
        "run:r1"
      ],
      "run": "r1",
      "subject": "which format?",
      "to": "run:r1",
      "type": "question"
    },
    {
      "body": "use json",
      "created_ms": 16,
      "from": "run:r1",
      "id": "m2",
      "priority": "high",
      "read_ms": 20,
      "recipients": [
        "attempt:a1"
      ],
      "reply_to": "m1",
      "run": "r1",
      "subject": "json",
      "thread": "m1",
      "to": "attempt:a1",
      "type": "answer"
    },
    {
      "body": "working",
      "body_text": {
        "kind": "next.working"
      },
      "created_ms": 17,
      "from": "run:r1",
      "id": "m3",
      "payload": {
        "list": [
          3,
          1,
          2
        ],
        "note": null
      },
      "priority": "normal",
      "read_ms": 20,
      "recipients": [
        "attempt:a1",
        "attempt:a2"
      ],
      "run": "r1",
      "subject": "group  check  (no verdict)",
      "text": {
        "kind": "group.checked",
        "params": {
          "n": "2"
        }
      },
      "to": "@all",
      "type": "status"
    },
    {
      "body": "",
      "created_ms": 17,
      "from": "run:r1",
      "id": "m4",
      "priority": "normal",
      "recipients": [
        "attempt:a2"
      ],
      "run": "r1",
      "subject": "idle only",
      "to": "@idle",
      "type": "status"
    },
    {
      "body": "",
      "created_ms": 17,
      "from": "attempt:a1",
      "id": "m5",
      "priority": "normal",
      "recipients": [
        "run:r1"
      ],
      "run": "r1",
      "subject": "alive",
      "to": "run:r1",
      "type": "heartbeat"
    },
    {
      "body": "parsed",
      "created_ms": 30,
      "from": "attempt:a1",
      "id": "m6",
      "payload": {
        "accepted": true,
        "attempt": "a1",
        "files": [
          "src/parse.rs"
        ],
        "generation": 1,
        "item": "i1",
        "outcome": "succeeded",
        "release_at_ms": 1030,
        "reopens": 0,
        "report": "report.md"
      },
      "priority": "normal",
      "recipients": [
        "run:r1"
      ],
      "run": "r1",
      "subject": "done (succeeded): Parse the header",
      "text": {
        "kind": "done.report",
        "params": {
          "attempt": "a1",
          "item": "i1",
          "outcome": "succeeded",
          "title": "Parse the header"
        }
      },
      "to": "run:r1",
      "type": "done"
    },
    {
      "body": "",
      "created_ms": 32,
      "from": "run:r1",
      "id": "m7",
      "priority": "normal",
      "recipients": [
        "attempt:a1"
      ],
      "run": "r1",
      "subject": "one more thing",
      "to": "attempt:a1",
      "type": "handoff"
    },
    {
      "body": "also done",
      "created_ms": 33,
      "from": "attempt:a1",
      "id": "m8",
      "payload": {
        "accepted": true,
        "attempt": "a1",
        "files": [],
        "generation": 1,
        "item": "i1",
        "outcome": "succeeded",
        "release_at_ms": null,
        "reopens": 1,
        "report": null
      },
      "priority": "normal",
      "recipients": [
        "run:r1"
      ],
      "run": "r1",
      "subject": "done (succeeded): Parse the header",
      "text": {
        "kind": "done.report",
        "params": {
          "attempt": "a1",
          "item": "i1",
          "outcome": "succeeded",
          "title": "Parse the header"
        }
      },
      "to": "run:r1",
      "type": "done"
    },
    {
      "body": "too late",
      "created_ms": 35,
      "fenced": true,
      "from": "attempt:a2",
      "id": "m9",
      "payload": {
        "accepted": false,
        "attempt": "a2",
        "files": [],
        "generation": 1,
        "item": "i3",
        "outcome": "failed",
        "release_at_ms": null,
        "reopens": 0,
        "report": null
      },
      "priority": "normal",
      "recipients": [
        "run:r1"
      ],
      "run": "r1",
      "subject": "late done (failed) from superseded attempt a2; ignored",
      "text": {
        "kind": "done.late",
        "params": {
          "attempt": "a2",
          "item": "i3",
          "outcome": "failed"
        }
      },
      "to": "run:r1",
      "type": "done"
    },
    {
      "body": "could not",
      "created_ms": 42,
      "from": "attempt:a6",
      "id": "m10",
      "payload": {
        "accepted": true,
        "attempt": "a6",
        "files": [],
        "generation": 1,
        "item": "i2",
        "outcome": "failed",
        "release_at_ms": null,
        "reopens": 0,
        "report": null
      },
      "priority": "normal",
      "recipients": [
        "run:r1"
      ],
      "run": "r1",
      "subject": "done (failed): wire it",
      "text": {
        "kind": "done.report",
        "params": {
          "attempt": "a6",
          "item": "i2",
          "outcome": "failed",
          "title": "wire it"
        }
      },
      "to": "run:r1",
      "type": "done"
    }
  ],
  "next_ids": [
    {
      "created_ms": 101,
      "depth": 0,
      "id": "r2",
      "objective": "next"
    },
    {
      "circuit_open": false,
      "created_ms": 102,
      "generation": 0,
      "id": "i5",
      "run": "r2",
      "spec": "next item",
      "start_failures": 0,
      "state": "ready",
      "title": "next item",
      "updated_ms": 102
    }
  ],
  "notes": [],
  "run": {
    "attempts": [
      {
        "agent": "codex",
        "body": "also done",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "depth": 1,
        "ended_ms": 33,
        "generation": 1,
        "id": "a1",
        "item": "i1",
        "log_path": "logs/a1.txt",
        "place": "new",
        "ready_ms": 12,
        "released_ms": 44,
        "reopens": 1,
        "run": "r1",
        "seen_turn": 3,
        "state": "succeeded",
        "task": "task_1",
        "terminal": "term_w1",
        "token_hash": "6ab9f1eb8f7d3388f4f9d586f66e99fd54080df2c446f0e58668b09c08a16dd0",
        "worker": "released"
      },
      {
        "agent": "codex",
        "created_ms": 13,
        "depth": 1,
        "ended_ms": 34,
        "error": "replaced by a retry",
        "generation": 1,
        "id": "a2",
        "item": "i3",
        "place": "new",
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "abandoned",
        "token_hash": "015f7e6bc5aeaf483724089e9252cc13b50951a6b69412522765cff4d780306e",
        "worker": "kept"
      },
      {
        "agent": "codex",
        "created_ms": 34,
        "depth": 1,
        "escalated_turn": 6,
        "generation": 2,
        "id": "a3",
        "item": "i3",
        "place": "new",
        "reopens": 0,
        "retry_of": "a2",
        "run": "r1",
        "seen_turn": 7,
        "state": "starting",
        "token_hash": "2f5052c9fd15b19a18c584d01363568198613f0c34e84409ef7938709a159ec2",
        "worker": "active"
      },
      {
        "agent": "codex",
        "created_ms": 37,
        "depth": 1,
        "ended_ms": 38,
        "error": "spawn failed",
        "generation": 1,
        "id": "a4",
        "item": "i4",
        "place": "new",
        "released_ms": 38,
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "start_failed",
        "token_hash": "94091dd64a21ffe94214bc6d17deeb43873a5cf2f0a71b4b5caa9a5c81b6967d",
        "worker": "released"
      },
      {
        "agent": "codex",
        "created_ms": 39,
        "depth": 1,
        "ended_ms": 40,
        "error": "terminal gone",
        "generation": 2,
        "id": "a5",
        "item": "i4",
        "place": "new",
        "release_at_ms": 540,
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "lost",
        "token_hash": "88dbf612972c594a2c2f6dde493ef1c4cd27adba8c36e6ebd00b89ff143aba61",
        "worker": "finished"
      },
      {
        "agent": "codex",
        "body": "could not",
        "created_ms": 42,
        "depth": 1,
        "ended_ms": 42,
        "generation": 1,
        "id": "a6",
        "item": "i2",
        "place": "new",
        "reopens": 0,
        "run": "r1",
        "seen_turn": 0,
        "state": "failed",
        "token_hash": "1d92ad4b6987fa0347cc5d2fb6cf9e47c83f4f6caeb3b5ef6f629730528921c3",
        "worker": "finished"
      }
    ],
    "items": [
      {
        "attempt": "a1",
        "circuit_open": false,
        "created_ms": 6,
        "generation": 1,
        "id": "i1",
        "run": "r1",
        "spec": "\n# Parse the header\n\nmore detail",
        "start_failures": 0,
        "state": "ready",
        "title": "Parse the header",
        "updated_ms": 42
      },
      {
        "attempt": "a6",
        "circuit_open": false,
        "created_ms": 7,
        "deps": [
          "i1"
        ],
        "generation": 1,
        "id": "i2",
        "run": "r1",
        "spec": "connect",
        "start_failures": 0,
        "state": "failed",
        "title": "wire it",
        "updated_ms": 42
      },
      {
        "attempt": "a3",
        "circuit_open": false,
        "created_ms": 8,
        "generation": 2,
        "id": "i3",
        "parent": "i1",
        "run": "r1",
        "spec": "a sub task",
        "start_failures": 0,
        "state": "assigned",
        "title": "child",
        "updated_ms": 36
      },
      {
        "attempt": "a5",
        "circuit_open": false,
        "created_ms": 37,
        "generation": 2,
        "id": "i4",
        "run": "r1",
        "spec": "fourth",
        "start_failures": 1,
        "state": "ready",
        "title": "fourth",
        "updated_ms": 40
      }
    ],
    "run": {
      "coordinator_terminal": "term_c",
      "created_ms": 5,
      "depth": 0,
      "id": "r1",
      "objective": "ship the parser"
    },
    "version": 1,
    "wakes": {
      "attempt:a3": {},
      "run:r1": {
        "notified_ms": 50
      }
    }
  },
  "wake": {
    "a3_pending": false,
    "a3_unconfirmed": 0,
    "run_notified": 50,
    "run_wants": false
  }
}"##;

const DEFAULTS: &str = r##"{
  "loaded": {
    "mail_of": [
      {
        "body": "",
        "created_ms": 4,
        "from": "run:r7",
        "id": "m3",
        "priority": "normal",
        "read_ms": 5,
        "recipients": [
          "attempt:a4"
        ],
        "run": "r7",
        "subject": "s",
        "to": "attempt:a4",
        "type": "status"
      },
      {
        "body": "finished",
        "created_ms": 6,
        "from": "attempt:a4",
        "id": "m4",
        "payload": {
          "accepted": true,
          "attempt": "a4",
          "files": [
            "a.rs"
          ],
          "generation": 1,
          "item": "i5",
          "outcome": "succeeded",
          "release_at_ms": 900,
          "reopens": 0,
          "report": null
        },
        "priority": "normal",
        "recipients": [
          "run:r7"
        ],
        "run": "r7",
        "subject": "done",
        "to": "run:r7",
        "type": "done"
      }
    ],
    "notes": [
      "run r7: applied the recorded done of a4"
    ],
    "run": {
      "attempts": [
        {
          "agent": "codex",
          "body": "finished",
          "created_ms": 3,
          "depth": 1,
          "ended_ms": 6,
          "files": [
            "a.rs"
          ],
          "generation": 1,
          "id": "a4",
          "item": "i5",
          "place": "new",
          "release_at_ms": 900,
          "reopens": 0,
          "run": "r7",
          "seen_turn": 0,
          "state": "succeeded",
          "worker": "finished"
        }
      ],
      "items": [
        {
          "attempt": "a4",
          "circuit_open": false,
          "created_ms": 2,
          "generation": 1,
          "id": "i5",
          "run": "r7",
          "spec": "s",
          "start_failures": 0,
          "state": "completed",
          "title": "t",
          "updated_ms": 6
        },
        {
          "circuit_open": false,
          "created_ms": 2,
          "deps": [
            "i5"
          ],
          "generation": 0,
          "id": "i6",
          "run": "r7",
          "spec": "s",
          "start_failures": 0,
          "state": "ready",
          "title": "u",
          "updated_ms": 6
        }
      ],
      "run": {
        "created_ms": 1,
        "depth": 0,
        "id": "r7",
        "objective": "old"
      },
      "version": 1,
      "wakes": {}
    },
    "run_json": {
      "attempts": [
        {
          "agent": "codex",
          "body": "finished",
          "created_ms": 3,
          "depth": 1,
          "ended_ms": 6,
          "files": [
            "a.rs"
          ],
          "generation": 1,
          "id": "a4",
          "item": "i5",
          "place": "new",
          "release_at_ms": 900,
          "reopens": 0,
          "run": "r7",
          "seen_turn": 0,
          "state": "succeeded",
          "worker": "finished"
        }
      ],
      "items": [
        {
          "attempt": "a4",
          "circuit_open": false,
          "created_ms": 2,
          "generation": 1,
          "id": "i5",
          "run": "r7",
          "spec": "s",
          "start_failures": 0,
          "state": "completed",
          "title": "t",
          "updated_ms": 6
        },
        {
          "circuit_open": false,
          "created_ms": 2,
          "deps": [
            "i5"
          ],
          "generation": 0,
          "id": "i6",
          "run": "r7",
          "spec": "s",
          "start_failures": 0,
          "state": "ready",
          "title": "u",
          "updated_ms": 6
        }
      ],
      "run": {
        "created_ms": 1,
        "depth": 0,
        "id": "r7",
        "objective": "old"
      },
      "version": 1,
      "wakes": {}
    },
    "unread_a4": []
  },
  "msg_text": [
    {
      "en": "done (succeeded): fix login",
      "text": {
        "kind": "done.report",
        "params": {
          "attempt": "a2",
          "item": "i1",
          "outcome": "succeeded",
          "title": "fix login"
        }
      }
    },
    {
      "en": "late done (succeeded) from superseded attempt a2; ignored",
      "text": {
        "kind": "done.late",
        "params": {
          "attempt": "a2",
          "item": "i1",
          "outcome": "succeeded",
          "title": "fix login"
        }
      }
    },
    {
      "en": "working",
      "text": {
        "kind": "next.working",
        "params": {
          "attempt": "a2",
          "item": "i1"
        }
      }
    }
  ],
  "next": [
    {
      "created_ms": 51,
      "depth": 0,
      "id": "r8",
      "objective": "new"
    },
    {
      "circuit_open": false,
      "created_ms": 52,
      "generation": 0,
      "id": "i7",
      "run": "r7",
      "spec": "more",
      "start_failures": 0,
      "state": "ready",
      "title": "more",
      "updated_ms": 52
    },
    {
      "body": "",
      "created_ms": 53,
      "from": "run:r7",
      "id": "m5",
      "priority": "normal",
      "recipients": [
        "run:r7"
      ],
      "run": "r7",
      "subject": "n",
      "to": "run:r7",
      "type": "status"
    }
  ]
}"##;
