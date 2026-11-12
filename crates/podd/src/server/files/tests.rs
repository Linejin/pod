//! TM-15/TM-16 on real Windows file objects: junctions, symbolic links
//! (when this account may create them), hard links, NTFS streams, 8.3
//! aliases, a folder swapped right after the check, and saves that crash
//! between the journal and the rename.

use super::*;
use crate::git::pin::junction;

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp(tag: &str) -> Tmp {
    let t = std::env::temp_dir().join(format!(
        "podd-files-{tag}-{}-{}",
        std::process::id(),
        crate::now_ms()
    ));
    let _ = std::fs::remove_dir_all(&t);
    std::fs::create_dir_all(&t).unwrap();
    Tmp(t)
}

/// A worktree `wt` with files, and an `outside` folder holding the canary.
fn fixture(tag: &str) -> (Tmp, PathBuf, PathBuf) {
    let t = tmp(tag);
    let wt = t.0.join("wt");
    let outside = t.0.join("outside");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::create_dir_all(wt.join(".git")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(wt.join(".git").join("config"), "[core]\n").unwrap();
    std::fs::write(wt.join("src").join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(outside.join("canary.txt"), "secret").unwrap();
    (t, wt, outside)
}

fn rel(p: &str) -> Rel {
    path::parse(p).unwrap()
}

fn read(root: &Path, p: &str) -> Result<Vec<u8>, Fault> {
    read_file(root, &rel(p), MAX_READ_BYTES)
}

fn canary_intact(outside: &Path) {
    assert_eq!(
        std::fs::read_to_string(outside.join("canary.txt")).unwrap(),
        "secret"
    );
    let names: Vec<_> = std::fs::read_dir(outside)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "nothing new outside: {names:?}");
}

fn save<'a>(
    task: &'a str,
    root: &'a Path,
    r: &'a Rel,
    data: &'a [u8],
    expected: &'a str,
    op: &'a str,
) -> Save<'a> {
    Save {
        task,
        root,
        rel: r,
        data,
        body: review::revision(data),
        expected,
        op,
    }
}

fn denied_code(r: Result<FsWriteResult, RpcError>) -> i64 {
    r.unwrap_err().code
}

/// TM-15: junctions, links and hard links are refused, never followed,
/// for reads, stat, listing and saves.
#[test]
fn links_junctions_and_hard_links_are_refused() {
    let (_t, wt, outside) = fixture("links");
    assert!(junction(&wt.join("jdir"), &outside), "mklink /J");
    std::fs::hard_link(outside.join("canary.txt"), wt.join("hard.txt")).unwrap();
    let symlink =
        std::os::windows::fs::symlink_file(outside.join("canary.txt"), wt.join("sym.txt")).is_ok();
    let symdir = std::os::windows::fs::symlink_dir(&outside, wt.join("symdir")).is_ok();

    assert!(matches!(
        read(&wt, "jdir/canary.txt"),
        Err(Fault::Denied(_))
    ));
    assert!(matches!(read(&wt, "jdir"), Err(Fault::Denied(_))));
    assert!(matches!(read(&wt, "hard.txt"), Err(Fault::Denied(m)) if m.contains("hard link")));
    if symlink {
        assert!(matches!(read(&wt, "sym.txt"), Err(Fault::Denied(_))));
    }
    if symdir {
        assert!(matches!(
            read(&wt, "symdir/canary.txt"),
            Err(Fault::Denied(_))
        ));
    }
    // Listing shows them as links; stat too; never entered.
    let l = list("r/t", &wt, &rel(""), &FsListParams::default()).unwrap();
    let kind = |n: &str| l.entries.iter().find(|e| e.name == n).map(|e| e.kind);
    assert_eq!(kind("jdir"), Some(FsKind::Link));
    assert_eq!(kind("src"), Some(FsKind::Dir));
    assert_eq!(kind(".git"), None, "never listed");
    assert!(list("r/t", &wt, &rel("jdir"), &FsListParams::default()).is_err());
    assert_eq!(
        stat("r/t", &wt, &rel("jdir")).unwrap().entry.unwrap().kind,
        FsKind::Link
    );
    let hs = stat("r/t", &wt, &rel("hard.txt")).unwrap();
    assert_eq!(hs.entry.unwrap().kind, FsKind::Link);
    assert!(hs.revision.is_none());
    // A save through any of them: refused, the canary untouched.
    let files = Files::load(&_t.0.join("state"));
    let rev = review::revision(b"secret");
    for p in ["jdir/canary.txt", "hard.txt"] {
        let r = rel(p);
        let code =
            denied_code(files.write(save("r/t", &wt, &r, b"pwned", &rev, "op-links-1"), &|_| {}));
        assert_eq!(code, files_codes::PATH_DENIED, "{p}");
    }
    // The worktree itself as a junction: refused.
    let jroot = _t.0.join("jroot");
    assert!(junction(&jroot, &wt));
    assert!(matches!(read(&jroot, "src/main.rs"), Err(Fault::Denied(_))));
    canary_intact(&outside);
}

/// TM-15: an NTFS stream, a device name, `.git` and an 8.3 alias of `.git`
/// or of any folder are refused; the main stream reads alone.
#[test]
fn streams_devices_git_and_aliases_are_refused() {
    let (_t, wt, _outside) = fixture("ads");
    std::fs::write(wt.join("a.txt"), "main").unwrap();
    std::fs::write(wt.join("a.txt:hidden"), "stream").unwrap();
    assert_eq!(read(&wt, "a.txt").unwrap(), b"main");
    for bad in [
        "a.txt:hidden",
        "a.txt::$DATA",
        "NUL",
        "src/aux.rs",
        ".git/config",
        "../wt/a.txt",
    ] {
        assert!(path::parse(bad).is_err(), "{bad}");
    }
    std::fs::create_dir_all(wt.join("Long Folder Name")).unwrap();
    std::fs::write(wt.join("Long Folder Name").join("x.txt"), "x").unwrap();
    assert_eq!(read(&wt, "long folder name/x.txt").unwrap(), b"x");
    for real in [".git", "Long Folder Name"] {
        if let Some(short) = crate::git::short_spelling(&wt.join(real)) {
            let alias = short.file_name().unwrap().to_string_lossy().into_owned();
            let inside = if real == ".git" { "config" } else { "x.txt" };
            let r = read(&wt, &format!("{alias}/{inside}"));
            assert!(matches!(r, Err(Fault::Denied(_))), "{alias}: {r:?}");
            assert!(stat("r/t", &wt, &rel(&alias)).is_err(), "{alias}");
        }
    }
}

/// TM-15 completion: a folder on the way is swapped for a junction right
/// after the check (the barrier), and the target is written by another
/// program: both are refused while podd holds them; the canary is never
/// read or written and the save lands in the worktree.
#[test]
fn a_swap_after_the_check_cannot_redirect_a_save() {
    let (t, wt, outside) = fixture("swap");
    std::fs::write(outside.join("main.rs"), "outside").unwrap();
    let files = Files::load(&t.0.join("state"));
    let old = std::fs::read(wt.join("src").join("main.rs")).unwrap();
    let r = rel("src/main.rs");
    let swapped = std::cell::Cell::new(Vec::<String>::new());
    let hook = |stage: &str| {
        assert_eq!(stage, "checked");
        let mut seen = Vec::new();
        let src = wt.join("src");
        if std::fs::rename(&src, wt.join("src-moved")).is_ok() {
            seen.push("folder renamed".to_string());
            if junction(&src, &outside) {
                seen.push("junction placed".into());
            }
        }
        if std::fs::remove_dir(&src).is_ok() {
            seen.push("folder deleted".into());
        }
        // Writers are shut out from the check on (deleting is shared: the
        // save's own rename needs it; a deleted target is simply saved).
        if std::fs::OpenOptions::new()
            .write(true)
            .open(src.join("main.rs"))
            .is_ok()
        {
            seen.push("target opened for writing".into());
        }
        if std::fs::rename(&wt, t.0.join("wt-moved")).is_ok() {
            seen.push("root renamed".into());
        }
        swapped.set(seen);
    };
    let res = files
        .write(
            save(
                "r/t",
                &wt,
                &r,
                b"saved\r\n",
                &review::revision(&old),
                "op-swap-0001",
            ),
            &hook,
        )
        .unwrap();
    assert_eq!(swapped.take(), Vec::<String>::new(), "every swap refused");
    assert_eq!(res.outcome, FsWriteOutcome::Written);
    assert_eq!(
        std::fs::read(wt.join("src").join("main.rs")).unwrap(),
        b"saved\r\n"
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("main.rs")).unwrap(),
        "outside"
    );
    let left: Vec<_> = std::fs::read_dir(wt.join("src"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 1, "no temp file left: {left:?}");
}

/// TM-16: a stale save is a conflict (nothing written); the same operation
/// twice writes once; the same id with other content is refused.
#[test]
fn stale_saves_conflict_and_operations_apply_once() {
    let (t, wt, _o) = fixture("cas");
    let files = Files::load(&t.0.join("state"));
    let p = wt.join("한글 파일.txt");
    let v1 = "\u{feff}첫 줄\r\n둘째\r\n".as_bytes().to_vec();
    std::fs::write(&p, &v1).unwrap();
    let r = rel("한글 파일.txt");
    let got = read(&wt, "한글 파일.txt").unwrap();
    assert_eq!(got, v1, "bytes as on disk (BOM, CRLF)");
    let rev1 = review::revision(&got);

    // Someone else changes the file after the client read it.
    std::fs::write(&p, "agent\r\n").unwrap();
    let e = files
        .write(
            save("r/t", &wt, &r, b"mine\r\n", &rev1, "op-cas-00001"),
            &|_| {},
        )
        .unwrap_err();
    assert_eq!(e.code, files_codes::CONFLICT);
    assert_eq!(
        e.data.as_ref().unwrap()["current"],
        review::revision(b"agent\r\n")
    );
    assert_eq!(std::fs::read(&p).unwrap(), b"agent\r\n", "not overwritten");

    let rev2 = review::revision(b"agent\r\n");
    let body = "\u{feff}수정\r\n".as_bytes();
    let first = files
        .write(save("r/t", &wt, &r, body, &rev2, "op-cas-00002"), &|_| {})
        .unwrap();
    assert_eq!(first.outcome, FsWriteOutcome::Written);
    assert_eq!(first.revision, review::revision(body));
    // The answer was lost; the client sends the same save again.
    let again = files
        .write(save("r/t", &wt, &r, body, &rev2, "op-cas-00002"), &|_| {})
        .unwrap();
    assert_eq!(again.outcome, FsWriteOutcome::Already);
    assert_eq!(std::fs::read(&p).unwrap(), body);
    // Same id, other content or other file: refused.
    let e = files
        .write(
            save("r/t", &wt, &r, b"other", &rev2, "op-cas-00002"),
            &|_| {},
        )
        .unwrap_err();
    assert_eq!(e.code, files_codes::OPERATION_MISMATCH);
    let r2 = rel("src/main.rs");
    let e = files
        .write(save("r/t", &wt, &r2, body, &rev2, "op-cas-00002"), &|_| {})
        .unwrap_err();
    assert_eq!(e.code, files_codes::OPERATION_MISMATCH);
    // The journal survives a restart.
    drop(files);
    let files = Files::load(&t.0.join("state"));
    let again = files
        .write(save("r/t", &wt, &r, body, &rev2, "op-cas-00002"), &|_| {})
        .unwrap();
    assert_eq!(again.outcome, FsWriteOutcome::Already);
    // A deleted file: conflict with no current revision; v1 never creates.
    std::fs::remove_file(&p).unwrap();
    let e = files
        .write(
            save(
                "r/t",
                &wt,
                &r,
                body,
                &review::revision(body),
                "op-cas-00003",
            ),
            &|_| {},
        )
        .unwrap_err();
    assert_eq!(e.code, files_codes::CONFLICT);
    assert!(e.data.unwrap()["current"].is_null());
    assert!(!p.exists());
}

/// TM-16: a crash between the journal and the end of the save is settled
/// at the next start from the file's content: written once, or not at
/// all, never twice; the temp file goes.
#[test]
fn a_crashed_save_is_settled_from_the_file() {
    let (t, wt, _o) = fixture("crash");
    let state = t.0.join("state");
    let p = wt.join("src").join("main.rs");
    let old = std::fs::read(&p).unwrap();
    let old_rev = review::revision(&old);
    let body = b"fn main() { saved(); }\n";
    let record = |op: &str, tmp: &str| Record {
        operation_id: op.into(),
        task: "r/t".into(),
        root: wt.display().to_string(),
        path: "src/main.rs".into(),
        body: review::revision(body),
        size: body.len() as u64,
        expected: old_rev.clone(),
        tmp: tmp.into(),
        phase: Phase::Prepared,
        at_ms: crate::now_ms(),
    };
    // Crash 1: journal written, temp file half written, no rename.
    {
        let mut j = Journal::load(&state.join("files").join("ops"), crate::now_ms());
        j.put(record("op-crash-0001", ".~pod-save-a.tmp")).unwrap();
    }
    std::fs::write(wt.join("src").join(".~pod-save-a.tmp"), b"fn ma").unwrap();
    let files = Files::load(&state);
    assert!(
        files.journal.lock().unwrap().get("op-crash-0001").is_none(),
        "not written: forgotten"
    );
    assert!(
        !wt.join("src").join(".~pod-save-a.tmp").exists(),
        "temp removed"
    );
    assert_eq!(std::fs::read(&p).unwrap(), old);
    // The client resends: applied now, once.
    let r = rel("src/main.rs");
    let res = files
        .write(
            save("r/t", &wt, &r, body, &old_rev, "op-crash-0001"),
            &|_| {},
        )
        .unwrap();
    assert_eq!(res.outcome, FsWriteOutcome::Written);
    drop(files);

    // Crash 2: renamed, but the journal still says prepared.
    std::fs::write(&p, &old).unwrap();
    {
        let mut j = Journal::load(&state.join("files").join("ops"), crate::now_ms());
        j.put(record("op-crash-0002", ".~pod-save-b.tmp")).unwrap();
    }
    std::fs::write(&p, body).unwrap();
    let files = Files::load(&state);
    assert_eq!(
        files
            .journal
            .lock()
            .unwrap()
            .get("op-crash-0002")
            .map(|r| r.phase),
        Some(Phase::Done)
    );
    let res = files
        .write(
            save("r/t", &wt, &r, body, &old_rev, "op-crash-0002"),
            &|_| {},
        )
        .unwrap();
    assert_eq!(res.outcome, FsWriteOutcome::Already, "never twice");
    assert_eq!(std::fs::read(&p).unwrap(), body);
}

/// Listing: sorted pages with a cursor, temp files hidden, Korean and
/// space names; stat of a missing path is no entry.
#[test]
fn listing_pages_and_stat() {
    let (_t, wt, _o) = fixture("list");
    for n in ["b.txt", "a b.txt", "한글.md", ".~pod-save-zz.tmp"] {
        std::fs::write(wt.join(n), n).unwrap();
    }
    let first = list(
        "r/t",
        &wt,
        &rel(""),
        &FsListParams {
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        first
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["a b.txt", "b.txt"]
    );
    let next = list(
        "r/t",
        &wt,
        &rel(""),
        &FsListParams {
            cursor: first.next.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        next.entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["src", "한글.md"]
    );
    assert!(next.next.is_none());
    let s = stat("r/t", &wt, &rel("한글.md")).unwrap();
    assert_eq!(s.entry.unwrap().kind, FsKind::File);
    assert_eq!(s.revision.unwrap(), review::revision("한글.md".as_bytes()));
    assert!(stat("r/t", &wt, &rel("nope/x")).unwrap().entry.is_none());
    assert!(matches!(read(&wt, "src"), Err(Fault::Kind("dir"))));
    assert!(matches!(
        read(&wt, "src/main.rs/x"),
        Err(Fault::Kind("file"))
    ));
}

/// The size guard: a file above the read limit is refused, not truncated.
#[test]
fn big_files_are_refused_whole() {
    let (_t, wt, _o) = fixture("big");
    std::fs::write(wt.join("big.bin"), vec![b'x'; MAX_READ_BYTES as usize + 1]).unwrap();
    assert_eq!(
        read(&wt, "big.bin"),
        Err(Fault::TooLarge(MAX_READ_BYTES + 1))
    );
    std::fs::write(wt.join("edge.bin"), vec![b'x'; MAX_READ_BYTES as usize]).unwrap();
    assert_eq!(read(&wt, "edge.bin").unwrap().len() as u64, MAX_READ_BYTES);
}
