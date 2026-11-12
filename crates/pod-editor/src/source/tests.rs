use std::sync::Mutex;

use super::*;

/// A remote stand-in: the daemon's `fs.write` rules over a map (revision
/// check, at most once per save id) plus a link that can be down or lose
/// the next answer.
#[derive(Default)]
struct FakeRemote {
    files: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    done: Mutex<std::collections::HashMap<String, Revision>>,
    down: Mutex<bool>,
    lose_next_answer: Mutex<bool>,
    writes: Mutex<usize>,
}

impl FakeRemote {
    fn put(&self, rel: &str, b: &[u8]) {
        self.files.lock().unwrap().insert(rel.into(), b.to_vec());
    }
    fn get(&self, rel: &str) -> Vec<u8> {
        self.files.lock().unwrap()[rel].clone()
    }
}

impl FileSource for FakeRemote {
    fn max_read_bytes(&self) -> u64 {
        4 * 1024 * 1024
    }
    fn read(&self, rel: &str) -> Result<(Vec<u8>, Revision), SourceError> {
        if *self.down.lock().unwrap() {
            return Err(SourceError::Unreachable("down".into()));
        }
        let b = self
            .files
            .lock()
            .unwrap()
            .get(rel)
            .cloned()
            .ok_or(SourceError::NotFound)?;
        if b.len() as u64 > self.max_read_bytes() {
            return Err(SourceError::TooLarge {
                size: b.len() as u64,
                limit: self.max_read_bytes(),
            });
        }
        let r = Revision::of(&b);
        Ok((b, r))
    }
    fn stat(&self, _: &str) -> Result<Option<Stat>, SourceError> {
        Ok(None)
    }
    fn list(&self, _: &str) -> Result<Vec<Entry>, SourceError> {
        Ok(Vec::new())
    }
    fn write(
        &self,
        rel: &str,
        bytes: &[u8],
        expected: &Revision,
        id: &SaveId,
    ) -> Result<Written, SourceError> {
        if *self.down.lock().unwrap() {
            return Err(SourceError::Unreachable("down".into()));
        }
        if let Some(r) = self.done.lock().unwrap().get(&id.0) {
            return Ok(Written {
                revision: r.clone(),
                already: true,
            });
        }
        let now = self.files.lock().unwrap().get(rel).map(|b| Revision::of(b));
        if now.as_ref() != Some(expected) {
            return Err(SourceError::Conflict { current: now });
        }
        *self.writes.lock().unwrap() += 1;
        self.put(rel, bytes);
        let r = Revision::of(bytes);
        self.done.lock().unwrap().insert(id.0.clone(), r.clone());
        if std::mem::take(&mut *self.lose_next_answer.lock().unwrap()) {
            return Err(SourceError::Unknown("answer lost".into()));
        }
        Ok(Written {
            revision: r,
            already: false,
        })
    }
}

/// Korean text, a BOM and CRLF come back byte for byte after an edit; a
/// file that mixes endings keeps every unedited line's ending.
#[test]
fn encodings_and_line_endings_survive_a_remote_round_trip() {
    let src = FakeRemote::default();
    src.put("한글 메모.txt", "\u{feff}첫 줄\r\n둘째 줄\r\n".as_bytes());
    let mut d = Document::open(&src, "한글 메모.txt").unwrap();
    assert_eq!(d.text(), "첫 줄\n둘째 줄\n");
    assert!(d.loaded().format.bom);
    d.set_text("첫 줄\n고친 줄\n");
    assert!(d.is_dirty());
    assert_eq!(d.save(&src), Saved::Written { already: false });
    assert_eq!(
        src.get("한글 메모.txt"),
        "\u{feff}첫 줄\r\n고친 줄\r\n".as_bytes()
    );
    assert!(!d.is_dirty());

    src.put("mixed.txt", b"a\r\nb\nc\r\n");
    let mut d = Document::open(&src, "mixed.txt").unwrap();
    // The unedited LF line keeps LF; the edited one takes the majority.
    d.set_text("A\nb\nc\n");
    d.save(&src);
    assert_eq!(src.get("mixed.txt"), b"A\r\nb\nc\r\n");

    // CP949 bytes: read-only as UTF-8, editable when reopened as EUC-KR.
    let (cp949, _, _) = encoding_rs::EUC_KR.encode("가나\r\n");
    src.put("cp.txt", &cp949);
    let d = Document::open(&src, "cp.txt").unwrap();
    assert!(d.read_only());
    let mut d = Document::open_as(&src, "cp.txt", encoding_rs::EUC_KR).unwrap();
    assert!(!d.read_only());
    d.set_text("가나다\n");
    d.save(&src);
    assert_eq!(
        src.get("cp.txt"),
        &encoding_rs::EUC_KR.encode("가나다\r\n").0[..]
    );
}

/// A remote change after the buffer read the file: a conflict, nothing
/// overwritten, the buffer kept; reload drops the edits.
#[test]
fn a_stale_save_is_a_conflict_not_an_overwrite() {
    let src = FakeRemote::default();
    src.put("a.rs", b"fn a() {}\n");
    let mut d = Document::open(&src, "a.rs").unwrap();
    d.set_text("fn mine() {}\n");
    src.put("a.rs", b"fn agent() {}\n");
    match d.save(&src) {
        Saved::Conflict { current } => {
            assert_eq!(current, Some(Revision::of(b"fn agent() {}\n")))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(src.get("a.rs"), b"fn agent() {}\n");
    assert_eq!(d.text(), "fn mine() {}\n", "the buffer stays");
    d.reload(&src).unwrap();
    assert_eq!(d.text(), "fn agent() {}\n");
    assert!(!d.is_dirty());
}

/// Disconnected: the buffer and the save stay; once back, the same save
/// (same id) goes out. A lost answer is resent and written once.
#[test]
fn a_disconnect_keeps_the_buffer_and_a_resend_writes_once() {
    let src = FakeRemote::default();
    src.put("a.txt", b"one\n");
    let mut d = Document::open(&src, "a.txt").unwrap();
    d.set_text("two\n");
    *src.down.lock().unwrap() = true;
    assert!(matches!(
        d.save(&src),
        Saved::Kept(SourceError::Unreachable(_))
    ));
    assert!(d.is_dirty() && d.has_pending_save());
    assert_eq!(d.text(), "two\n");
    *src.down.lock().unwrap() = false;
    *src.lose_next_answer.lock().unwrap() = true;
    assert!(matches!(d.save(&src), Saved::Kept(SourceError::Unknown(_))));
    assert_eq!(src.get("a.txt"), b"two\n", "the lost answer's save landed");
    assert_eq!(d.save(&src), Saved::Written { already: true });
    assert_eq!(*src.writes.lock().unwrap(), 1, "written once");
    assert!(!d.is_dirty());
    assert_eq!(d.save(&src), Saved::Unchanged);
}

/// Size guard and binary files, as a local tab: too large or binary does
/// not open; above the large mark opens read-only.
#[test]
fn big_and_binary_files_follow_the_local_rules() {
    let src = FakeRemote::default();
    src.put("big.log", &vec![b'x'; 4 * 1024 * 1024 + 1]);
    assert!(matches!(
        Document::open(&src, "big.log"),
        Err(OpenError::Load(LoadError::TooLarge(_)))
    ));
    src.put("bin.dat", &[0, 1, 2]);
    assert!(matches!(
        Document::open(&src, "bin.dat"),
        Err(OpenError::Load(LoadError::Binary))
    ));
    src.put(
        "large.txt",
        &vec![b'y'; text::LARGE_FILE_BYTES as usize + 1],
    );
    let mut d = Document::open(&src, "large.txt").unwrap();
    assert!(d.read_only());
    d.set_text("z");
    assert!(matches!(d.save(&src), Saved::Refused(_)));
}

/// The local disk behind the same seam: the same revision as the daemon's,
/// a stale save refused, `..` refused.
#[test]
fn local_files_behind_the_seam() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a b.txt"), "x\r\n").unwrap();
    let src = LocalFiles::new(dir.path());
    let mut d = Document::open(&src, "a b.txt").unwrap();
    assert_eq!(d.revision(), &Revision::of(b"x\r\n"));
    assert_eq!(d.revision().0.len(), 64, "SHA-256 hex, like the daemon's");
    d.set_text("y\n");
    assert_eq!(d.save(&src), Saved::Written { already: false });
    assert_eq!(std::fs::read(dir.path().join("a b.txt")).unwrap(), b"y\r\n");
    d.set_text("z\n");
    std::fs::write(dir.path().join("a b.txt"), "other\r\n").unwrap();
    assert!(matches!(d.save(&src), Saved::Conflict { .. }));
    assert!(matches!(src.read("../x"), Err(SourceError::Denied(_))));
    assert_eq!(src.list("").unwrap()[0].name, "a b.txt");
}
