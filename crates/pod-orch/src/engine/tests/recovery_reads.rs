//! B10: an incomplete recovery must not overwrite unread state or reuse IDs.
use super::*;

#[test]
fn recovery_read_errors_are_diagnosed_and_block_writes_until_reload() {
    for file in ["index.json", "r1/run.json", "r1/mail.jsonl"] {
        let mut t = setup("read-error");
        t.o.create_run("original", None, None, 1).unwrap();
        let path = t.dir.join(file);
        let original = std::fs::read(&path).ok();
        if original.is_some() {
            std::fs::remove_file(&path).unwrap();
        }
        // A directory cannot be read as a snapshot/mail file on Windows.
        std::fs::create_dir(&path).unwrap();
        let mut recovered = Orch::load(&t.dir, Limits::default(), 2);
        assert!(!recovered.notes.is_empty(), "silent read failure: {file}");
        std::fs::remove_dir(&path).unwrap();
        if let Some(bytes) = original {
            std::fs::write(&path, bytes).unwrap();
        }
        let before = std::fs::read(store::index_json(&t.dir)).unwrap();
        assert_eq!(
            recovered
                .create_run("unsafe", None, None, 3)
                .unwrap_err()
                .kind,
            ErrorKind::Io
        );
        assert_eq!(
            recovered
                .remember("q", "create", serde_json::json!({}), 3)
                .unwrap_err()
                .kind,
            ErrorKind::Io
        );
        assert_eq!(std::fs::read(store::index_json(&t.dir)).unwrap(), before);
        assert_eq!(recovered.runs().count(), usize::from(file != "r1/run.json"));
        let mut reloaded = Orch::load(&t.dir, Limits::default(), 4);
        assert!(reloaded.notes.is_empty(), "{:?}", reloaded.notes);
        assert_eq!(reloaded.create_run("safe", None, None, 5).unwrap().id, "r2");
    }
}

#[test]
fn recovery_directory_error_is_not_an_empty_store() {
    let dir = temp("read-dir-error");
    std::fs::write(&dir, b"not a directory").unwrap();
    let mut recovered = Orch::load(&dir, Limits::default(), 1);
    assert!(!recovered.notes.is_empty());
    std::fs::remove_file(&dir).unwrap();
    assert_eq!(
        recovered
            .create_run("unsafe", None, None, 2)
            .unwrap_err()
            .kind,
        ErrorKind::Io
    );
    assert!(
        !dir.exists(),
        "a later successful write must not hide failed recovery"
    );
}

#[test]
fn recovery_missing_store_is_empty_but_existing_run_ids_are_reserved() {
    let mut t = setup("missing-snapshot");
    assert!(t.o.notes.is_empty());
    std::fs::create_dir_all(t.dir.join("r7")).unwrap();
    t.o = Orch::load(&t.dir, Limits::default(), 2);
    assert_eq!(t.o.create_run("new", None, None, 3).unwrap().id, "r8");
}

#[cfg(windows)]
#[test]
fn recovery_sharing_violation_keeps_original_bytes_and_blocks_mutation() {
    use std::os::windows::fs::OpenOptionsExt;
    for file in ["index.json", "r1/run.json", "r1/mail.jsonl"] {
        let mut t = setup("sharing-violation");
        t.o.create_run("original", None, None, 1).unwrap();
        t.o.add_item("r1", None, "work", &[], None, 1).unwrap();
        let path = t.dir.join(file);
        if !path.exists() {
            std::fs::write(&path, b"").unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let mut recovered = Orch::load(&t.dir, Limits::default(), 2);
        assert!(!recovered.notes.is_empty(), "{file}");
        drop(lock);
        assert_eq!(
            recovered
                .create_run("unsafe", None, None, 3)
                .unwrap_err()
                .kind,
            ErrorKind::Io
        );
        assert!(recovered.request("unknown", "create").is_err());
        if file != "r1/run.json" {
            assert_eq!(
                recovered.block_item("i1", "gate", 3).unwrap_err().kind,
                ErrorKind::Io
            );
            assert_eq!(recovered.item("i1").unwrap().state, ItemState::Ready);
            assert_eq!(
                recovered.ack("run:r1", &[], 3).unwrap_err().kind,
                ErrorKind::Io
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut reloaded = Orch::load(&t.dir, Limits::default(), 4);
        assert!(reloaded.notes.is_empty(), "{:?}", reloaded.notes);
        assert_eq!(reloaded.create_run("safe", None, None, 5).unwrap().id, "r2");
    }
}

#[test]
fn recovery_incomplete_defers_journal_snapshot_replay_until_reload() {
    let mut t = setup("incomplete-journal");
    t.o.create_run("original", None, None, 1).unwrap();
    t.o.add_item("r1", None, "work", &[], None, 1).unwrap();
    begin(&mut t.o, "r1", "i1", "token", None).unwrap();
    let snapshot = store::run_json(&t.dir, "r1");
    let before = std::fs::read(&snapshot).unwrap();
    let blocked_write = snapshot.with_extension("json.tmp");
    std::fs::create_dir(&blocked_write).unwrap();
    assert_eq!(
        t.o.done("a1", Some("token"), true, "done", &[], None, None, 2)
            .unwrap_err()
            .kind,
        ErrorKind::Io
    );
    std::fs::remove_dir(&blocked_write).unwrap();
    // Another unreadable snapshot must fence the store regardless of directory order.
    let blocked_read = store::run_json(&t.dir, "r2");
    std::fs::create_dir_all(&blocked_read).unwrap();
    let mut recovered = Orch::load(&t.dir, Limits::default(), 3);
    assert!(
        recovered
            .notes
            .iter()
            .any(|note| note.contains("cannot read snapshot"))
    );
    assert_eq!(std::fs::read(&snapshot).unwrap(), before);
    assert_eq!(
        recovered
            .done("a1", Some("token"), true, "done", &[], None, None, 4)
            .unwrap_err()
            .kind,
        ErrorKind::Io
    );
    std::fs::remove_dir(&blocked_read).unwrap();
    let mut reloaded = Orch::load(&t.dir, Limits::default(), 5);
    assert_eq!(reloaded.item("i1").unwrap().state, ItemState::Completed);
    assert!(
        reloaded
            .done("a1", Some("token"), true, "done", &[], None, None, 6)
            .unwrap()
            .repeated
    );
    assert_eq!(done_mails(&reloaded, "r1").len(), 1);
    assert!(Orch::load(&t.dir, Limits::default(), 7).notes.is_empty());
}
