//! B11: durable link-event deduplication across ambiguous journal writes.
use super::*;
use std::io::Write;

fn link(run: &str) -> NewMail {
    NewMail::pod(
        "pod",
        run_mailbox(run),
        MailType::Status,
        text::MsgText::new(text::REVIEW_DELIVERED),
    )
    .payload(serde_json::json!({"review":"d1", "task":"fixture/task"}))
}

#[test]
fn link_event_is_once_per_run_and_survives_restart_and_read_receipts() {
    let mut t = setup("link-once");
    for _ in 0..2 {
        t.o.create_run("run", None, None, 1).unwrap();
    }
    let (first, created) =
        t.o.send_link_once("r1", "review:d1", link("r1"), 2)
            .unwrap();
    assert!(created);
    let (again, created) =
        t.o.send_link_once("r1", "review:d1", link("r1"), 3)
            .unwrap();
    assert!(!created);
    assert_eq!(first, again);
    assert!(
        t.o.send_link_once("r2", "review:d1", link("r2"), 3)
            .unwrap()
            .1
    );
    assert!(
        t.o.send_link_once("r1", "review:d2", link("r1"), 3)
            .unwrap()
            .1
    );
    t.o.ack("run:r1", std::slice::from_ref(&first.id), 4)
        .unwrap();
    t.o = Orch::load(&t.dir, Limits::default(), 5);
    assert_eq!(
        t.o.send_link_once("r1", "review:d1", link("r1"), 6)
            .unwrap(),
        (first, false)
    );
    assert_eq!(t.o.mail_of("r1").unwrap().len(), 2);
    assert_eq!(t.o.unread("run:r1").len(), 1);
}

#[test]
fn link_partial_append_is_repaired_before_other_mail_and_retry() {
    let mut t = setup("link-partial");
    t.o.create_run("run", None, None, 1).unwrap();
    let error =
        t.o.send_link_once_with("r1", "review:d1", link("r1"), 2, |path, line| {
            let bytes = serde_json::to_vec(line).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            file.write_all(&bytes[..bytes.len() / 2])?;
            file.flush()?;
            Err(std::io::Error::other("injected partial append"))
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Io);
    assert!(t.o.mail_of("r1").unwrap().is_empty());
    assert!(
        std::fs::read(store::mail_jsonl(&t.dir, "r1"))
            .unwrap()
            .is_empty()
    );
    t.o.send(
        Some("r1"),
        NewMail::new("human", "run:r1", MailType::Status, "unrelated"),
        |_| false,
        3,
    )
    .unwrap();
    assert!(
        t.o.send_link_once("r1", "review:d1", link("r1"), 4)
            .unwrap()
            .1
    );
    let loaded = store::load_mail(&store::mail_jsonl(&t.dir, "r1")).unwrap();
    assert_eq!(
        (
            loaded.lines.len(),
            loaded.skipped_lines,
            loaded.truncated_bytes
        ),
        (2, 0, 0)
    );
    t.o = Orch::load(&t.dir, Limits::default(), 5);
    assert!(
        !t.o.send_link_once("r1", "review:d1", link("r1"), 6)
            .unwrap()
            .1
    );
}

#[test]
fn link_complete_append_before_error_is_not_duplicated_in_process_or_after_restart() {
    for restart in [false, true] {
        let mut t = setup("link-complete-error");
        t.o.create_run("run", None, None, 1).unwrap();
        let error =
            t.o.send_link_once_with("r1", "review:d1", link("r1"), 2, |path, line| {
                store::append_mail(path, line)?;
                Err(std::io::Error::other(
                    "injected failure after complete line",
                ))
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Io);
        assert_eq!(
            t.o.unread("run:r1").len(),
            1,
            "ambiguous complete mail is reconciled"
        );
        if restart {
            t.o = Orch::load(&t.dir, Limits::default(), 3);
        }
        let (mail, created) =
            t.o.send_link_once("r1", "review:d1", link("r1"), 4)
                .unwrap();
        assert!(!created);
        assert_eq!(mail.id, "m1");
        assert_eq!(t.o.mail_of("r1").unwrap().len(), 1);
        assert_eq!(
            store::load_mail(&store::mail_jsonl(&t.dir, "r1"))
                .unwrap()
                .lines
                .len(),
            1
        );
    }
}

#[test]
fn link_worker_payload_cannot_claim_a_pod_event_or_other_destination() {
    let mut t = setup("link-forged");
    t.o.create_run("run", None, None, 1).unwrap();
    let mut forged = link("r1");
    forged.from = "attempt:a1".into();
    forged.payload.as_mut().unwrap()["pod_link_event"] = serde_json::json!("review:d1");
    t.o.send(Some("r1"), forged.clone(), |_| false, 2).unwrap();
    assert!(
        t.o.send_link_once("r1", "review:d1", link("r1"), 3)
            .unwrap()
            .1
    );
    assert_eq!(
        t.o.send_link_once("r1", "review:d2", forged, 4)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidParams
    );
    assert_eq!(
        t.o.send_link_once("r1", "review:d2", link("r2"), 4)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidParams
    );
    assert_eq!(t.o.mail_of("r1").unwrap().len(), 2);
}

#[cfg(windows)]
#[test]
fn link_unresolved_tail_repair_fences_appends_without_erasing_evidence() {
    use std::os::windows::fs::OpenOptionsExt;
    let mut t = setup("link-repair-error");
    t.o.create_run("run", None, None, 1).unwrap();
    let mut held = None;
    let error =
        t.o.send_link_once_with("r1", "review:d1", link("r1"), 2, |path, line| {
            store::append_mail(path, line)?;
            held = Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(path)?,
            );
            Err(std::io::Error::other(
                "injected ambiguous write with unreadable journal",
            ))
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Io);
    drop(held);
    let path = store::mail_jsonl(&t.dir, "r1");
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(
        t.o.send(
            Some("r1"),
            NewMail::new("human", "run:r1", MailType::Status, "must not append"),
            |_| false,
            3
        )
        .unwrap_err()
        .kind,
        ErrorKind::Io
    );
    assert_eq!(
        t.o.send_link_once("r1", "review:d1", link("r1"), 3)
            .unwrap_err()
            .kind,
        ErrorKind::Io
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    t.o = Orch::load(&t.dir, Limits::default(), 4);
    assert!(
        !t.o.send_link_once("r1", "review:d1", link("r1"), 5)
            .unwrap()
            .1
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
