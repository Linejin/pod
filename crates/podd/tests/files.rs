//! docs/17 R5 at the daemon (protocol v22): `fs.*` and `review.diff` /
//! `review.blob` on a task's worktree through the RPC (docs/22 TM-15,
//! TM-16): the path matrix, a stale save, a save sent twice, saves cut by a
//! daemon crash at each point and settled at the next start, the size
//! guards, older hellos, and a repository whose diff/textconv/filter
//! programs never run.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use pod_test_support::git::git_ok;
use podd::client::{Client, ClientError};
use podd::protocol::*;
use serde_json::{Value, json};

struct Repo {
    root: PathBuf,
    path: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("files"));
        let path = root.join("repo");
        std::fs::create_dir_all(path.join("src")).unwrap();
        git_ok(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git_ok(&path, &["config", k, v]);
        }
        std::fs::write(path.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(path.join("한글 메모.txt"), "\u{feff}첫 줄\r\n둘째\r\n").unwrap();
        std::fs::write(path.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(path.join(".gitattributes"), "*.rs diff=mark filter=mark\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "init"]);
        Self { root, path }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A shell task `files` of `repo` on `d`; its worktree.
fn task(c: &mut Client, repo: &Repo) -> (String, PathBuf) {
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path.display().to_string(),
        name: Some("demo".into()),
    })
    .expect("repo.add");
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "demo".into(),
            name: "files".into(),
            agent: Some(AgentSpec::Kind(AgentKind::Shell)),
            ..Default::default()
        })
        .expect("task.create");
    (t.id, PathBuf::from(t.worktree))
}

fn read(c: &mut Client, task: &str, path: &str) -> Result<FsReadResult, ClientError> {
    c.call::<FsRead>(&FsPathParams {
        task: task.into(),
        path: path.into(),
    })
}

fn write(
    c: &mut Client,
    task: &str,
    path: &str,
    data: &[u8],
    expected: &str,
    op: &str,
) -> Result<FsWriteResult, ClientError> {
    c.call::<FsWrite>(&FsWriteParams {
        task: task.into(),
        path: path.into(),
        data: b64_encode(data),
        expected_revision: expected.into(),
        operation_id: op.into(),
    })
}

fn rpc_err(r: Result<impl std::fmt::Debug, ClientError>) -> RpcError {
    match r {
        Err(ClientError::Rpc(e)) => e,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

fn bytes(r: &FsReadResult) -> Vec<u8> {
    b64_decode(&r.data).unwrap()
}

/// Reads keep every byte (BOM, CRLF, Korean, spaces); a save replaces the
/// file only at the revision it was read at; a resend writes once; the
/// listing hides `.git`.
#[test]
fn reads_and_saves_over_the_daemon() {
    let repo = Repo::new();
    let d = Daemon::start("files-rw");
    let mut c = d.client();
    let h = c
        .hello(&[capability::REMOTE_FILES, capability::REMOTE_DIFF])
        .unwrap();
    assert_eq!(h.protocol, 22);
    assert_eq!(h.limits.max_read_bytes, Some(MAX_READ_BYTES));
    assert_eq!(h.limits.max_write_bytes, Some(MAX_WRITE_BYTES));
    let (t, wt) = task(&mut c, &repo);

    let r = read(&mut c, &t, "한글 메모.txt").unwrap();
    assert_eq!(bytes(&r), "\u{feff}첫 줄\r\n둘째\r\n".as_bytes());
    assert!(r.bom && r.utf8 && !r.binary);
    assert_eq!(r.eol, FsEol::Crlf);
    let l = c
        .call::<FsList>(&FsListParams {
            task: t.clone(),
            ..Default::default()
        })
        .unwrap();
    let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"src") && names.contains(&"한글 메모.txt"),
        "{names:?}"
    );
    assert!(!names.contains(&".git"));

    // Saved at the revision read: written, bytes exact.
    let new = "\u{feff}첫 줄\r\n고침\r\n".as_bytes();
    let w = write(
        &mut c,
        &t,
        "한글 메모.txt",
        new,
        &r.revision,
        "op-rw-000001",
    )
    .unwrap();
    assert_eq!(w.outcome, FsWriteOutcome::Written);
    assert_eq!(std::fs::read(wt.join("한글 메모.txt")).unwrap(), new);
    // The answer "was lost": the same save again is not written again.
    let again = write(
        &mut c,
        &t,
        "한글 메모.txt",
        new,
        &r.revision,
        "op-rw-000001",
    )
    .unwrap();
    assert_eq!(again.outcome, FsWriteOutcome::Already);
    assert_eq!(again.revision, w.revision);
    // Same id, other content: refused.
    let e = rpc_err(write(
        &mut c,
        &t,
        "한글 메모.txt",
        b"x",
        &r.revision,
        "op-rw-000001",
    ));
    assert_eq!(e.code, files_codes::OPERATION_MISMATCH);

    // Concurrent modification: an agent writes after the client read.
    let before = read(&mut c, &t, "src/main.rs").unwrap();
    std::fs::write(wt.join("src").join("main.rs"), "fn agent() {}\n").unwrap();
    let e = rpc_err(write(
        &mut c,
        &t,
        "src/main.rs",
        b"fn mine() {}\n",
        &before.revision,
        "op-rw-000002",
    ));
    assert_eq!(e.code, files_codes::CONFLICT);
    let now = read(&mut c, &t, "src/main.rs").unwrap();
    assert_eq!(e.data.unwrap()["current"], json!(now.revision));
    assert_eq!(bytes(&now), b"fn agent() {}\n", "never overwritten");

    // stat
    let s = c
        .call::<FsStat>(&FsPathParams {
            task: t.clone(),
            path: "src/main.rs".into(),
        })
        .unwrap();
    assert_eq!(s.entry.unwrap().kind, FsKind::File);
    assert_eq!(s.revision.as_deref(), Some(now.revision.as_str()));
    // Not a file / missing / v1 never creates.
    assert_eq!(
        rpc_err(read(&mut c, &t, "src")).code,
        files_codes::NOT_A_FILE
    );
    assert_eq!(
        rpc_err(read(&mut c, &t, "nope.txt")).code,
        files_codes::NOT_FOUND
    );
    let e = rpc_err(write(
        &mut c,
        &t,
        "new.txt",
        b"x",
        &now.revision,
        "op-rw-000003",
    ));
    assert_eq!(e.code, files_codes::CONFLICT);
    assert!(!wt.join("new.txt").exists());
    // Bad params.
    let e = rpc_err(write(
        &mut c,
        &t,
        "src/main.rs",
        b"x",
        &now.revision,
        "short",
    ));
    assert_eq!(e.code, codes::INVALID_PARAMS);
    let e = rpc_err(write(
        &mut c,
        &t,
        "src/main.rs",
        b"x",
        "abc",
        "op-rw-000004",
    ));
    assert_eq!(e.code, codes::INVALID_PARAMS);
}

/// TM-15 over the RPC: every escape spelling and every link kind is
/// refused with PATH_DENIED; the canary outside is never read or written.
#[test]
fn the_path_matrix_is_refused() {
    let repo = Repo::new();
    let d = Daemon::start("files-paths");
    let mut c = d.client();
    let (t, wt) = task(&mut c, &repo);
    let outside = repo.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("canary.txt"), "secret").unwrap();
    let canary = outside.join("canary.txt");
    let ok = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(wt.join("jdir"))
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "mklink /J");
    std::fs::hard_link(&canary, wt.join("hard.txt")).unwrap();
    std::fs::write(wt.join("a.txt"), "main").unwrap();
    std::fs::write(wt.join("a.txt:hidden"), "stream").unwrap();
    let paths = vec![
        "..\\outside\\canary.txt".to_string(),
        "../outside/canary.txt".into(),
        "src/../../outside/canary.txt".into(),
        canary.display().to_string(),
        "C:canary.txt".into(),
        "\\\\?\\".to_string() + &canary.display().to_string(),
        "\\\\.\\pipe\\x".into(),
        "\\\\localhost\\c$\\x".into(),
        "/outside/canary.txt".into(),
        "a.txt:hidden".into(),
        "a.txt::$DATA".into(),
        "NUL".into(),
        "src/con.txt".into(),
        "a.txt.".into(),
        ".git/config".into(),
        ".GIT/HEAD".into(),
        "jdir/canary.txt".into(),
        "jdir".into(),
        "hard.txt".into(),
        "a\u{0}b".into(),
    ];
    let rev = read(&mut c, &t, "a.txt").unwrap().revision;
    for (i, p) in paths.iter().enumerate() {
        let e = rpc_err(read(&mut c, &t, p));
        assert_eq!(
            e.code,
            files_codes::PATH_DENIED,
            "read {p:?}: {}",
            e.message
        );
        let e = rpc_err(write(
            &mut c,
            &t,
            p,
            b"pwned",
            &rev,
            &format!("op-matrix-{i:04}"),
        ));
        assert_eq!(
            e.code,
            files_codes::PATH_DENIED,
            "write {p:?}: {}",
            e.message
        );
        // Blobs come from git's objects, never the disk: refused, or no
        // such path in the index.
        match c.call::<ReviewBlob>(&ReviewBlobParams {
            task: t.clone(),
            rev: String::new(),
            path: p.clone(),
        }) {
            Ok(b) => assert!(b.data.is_none(), "blob {p:?}: {b:?}"),
            Err(ClientError::Rpc(e)) => {
                assert_eq!(
                    e.code,
                    files_codes::PATH_DENIED,
                    "blob {p:?}: {}",
                    e.message
                )
            }
            Err(e) => panic!("blob {p:?}: {e}"),
        }
    }
    assert_eq!(bytes(&read(&mut c, &t, "a.txt").unwrap()), b"main");
    assert_eq!(std::fs::read_to_string(&canary).unwrap(), "secret");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
}

/// The size guards: a big file is refused whole, a big write never reaches
/// the disk (and the connection stays usable).
#[test]
fn big_files_are_guarded() {
    let repo = Repo::new();
    let d = Daemon::start("files-big");
    let mut c = d.client();
    let (t, wt) = task(&mut c, &repo);
    std::fs::write(wt.join("big.log"), vec![b'x'; MAX_READ_BYTES as usize + 1]).unwrap();
    let e = rpc_err(read(&mut c, &t, "big.log"));
    assert_eq!(e.code, files_codes::TOO_LARGE);
    assert_eq!(e.data.unwrap()["limit"], MAX_READ_BYTES);
    let r = read(&mut c, &t, "src/main.rs").unwrap();
    let big = vec![b'y'; MAX_WRITE_BYTES as usize + 1];
    let e = rpc_err(write(
        &mut c,
        &t,
        "src/main.rs",
        &big,
        &r.revision,
        "op-big-00001",
    ));
    assert_eq!(e.code, files_codes::TOO_LARGE);
    assert_eq!(
        std::fs::read(wt.join("src").join("main.rs")).unwrap(),
        b"fn main() {}\n"
    );
    // The largest write that fits is saved.
    let edge = vec![b'z'; MAX_WRITE_BYTES as usize];
    let w = write(
        &mut c,
        &t,
        "src/main.rs",
        &edge,
        &r.revision,
        "op-big-00002",
    )
    .unwrap();
    assert_eq!(w.size, MAX_WRITE_BYTES);
}

/// TM-16: the daemon dies right after journaling a save (`prepared`) or
/// right after replacing the file (`renamed`); the next daemon settles it,
/// and the client's resend of the same save gives one result.
#[test]
fn a_save_cut_by_a_crash_applies_at_most_once() {
    let repo = Repo::new();
    let name = unique("files-crash");
    let pipe = format!(r"\\.\pipe\{name}");
    let state = std::env::temp_dir().join(&name);
    let path = "src/main.rs";
    let body = b"fn main() { saved(); }\n";
    for (stage, op, want) in [
        ("prepared", "op-crash-prepared", FsWriteOutcome::Written),
        ("renamed", "op-crash-renamed", FsWriteOutcome::Already),
    ] {
        let mut d = Daemon::start_full(
            pipe.clone(),
            state.clone(),
            &[],
            &[("POD_TEST_FILES_CRASH", stage)],
        );
        let mut c = d.client();
        let (t, wt) = if stage == "prepared" {
            task(&mut c, &repo)
        } else {
            let t = c
                .call::<TaskShow>(&TaskRef {
                    task: "demo/files".into(),
                })
                .unwrap();
            (t.id, PathBuf::from(t.worktree))
        };
        let target = wt.join("src").join("main.rs");
        std::fs::write(&target, "fn main() {}\n").unwrap();
        let r = read(&mut c, &t, path).unwrap();
        let res = write(&mut c, &t, path, body, &r.revision, op);
        assert!(
            matches!(res, Err(ClientError::Io(_) | ClientError::Protocol(_))),
            "{stage}: {res:?}"
        );
        d.wait_exit();
        // Dead: restart on the same pipe and state (keep them on drop).
        let (p, s) = (d.pipe.clone(), state.clone());
        std::mem::forget(d);
        let on_disk = std::fs::read(&target).unwrap();
        if stage == "prepared" {
            assert_eq!(on_disk, b"fn main() {}\n", "not replaced yet");
        } else {
            assert_eq!(on_disk, body, "replaced, not journaled as done");
        }
        let d = Daemon::start_on(p, s);
        let mut c = d.client();
        let again = write(&mut c, &t, path, body, &r.revision, op).unwrap();
        assert_eq!(again.outcome, want, "{stage}");
        assert_eq!(std::fs::read(&target).unwrap(), body);
        let again = write(&mut c, &t, path, body, &r.revision, op).unwrap();
        assert_eq!(
            again.outcome,
            FsWriteOutcome::Already,
            "{stage}: never twice"
        );
        let temps: Vec<_> = std::fs::read_dir(wt.join("src"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".~pod-save-"))
            .collect();
        assert!(temps.is_empty(), "{stage}: {temps:?}");
        let (p, s) = d.stop(true);
        assert_eq!((p.as_str(), s.as_path()), (pipe.as_str(), state.as_path()));
    }
    let _ = std::fs::remove_dir_all(&state);
}

/// Older hellos: no file capability or limits (downgraded on every
/// message, also `server.status` `endpoint` for v20), and a client that
/// speaks 21 cannot require `remote.files`.
#[test]
fn older_hellos_get_the_older_shapes() {
    let d = Daemon::start("files-hello");
    for (max, caps) in [(20u32, vec!["attach.chunks"]), (21, vec!["attach.chunks"])] {
        let mut c = d.client();
        let h: Value = c
            .call_raw(
                "connection.hello",
                json!({ "min_protocol": 12, "max_protocol": max }),
            )
            .unwrap();
        assert_eq!(h["protocol"], max);
        assert_eq!(h["capabilities"], json!(caps));
        assert!(h["limits"].get("max_read_bytes").is_none(), "{h}");
        let st: Value = c.call_raw("server.status", json!({})).unwrap();
        assert_eq!(st.get("endpoint").is_some(), max >= 21, "v{max}: {st}");
    }
    let mut c = d.client();
    let e = c
        .call_raw(
            "connection.hello",
            json!({ "min_protocol": 12, "max_protocol": 21, "require": ["remote.files"] }),
        )
        .unwrap_err();
    assert!(
        matches!(e, ClientError::Rpc(ref e) if e.code == remote_codes::PROTOCOL_MISMATCH),
        "{e:?}"
    );
    let h = d.client().hello(&[]).unwrap();
    assert!(h.capabilities.iter().any(|c| c == capability::REMOTE_DIFF));
}

/// The marker script: appends its name to `marker` and passes its input
/// through (as textconv, clean or smudge would).
fn marker_script(dir: &Path, marker: &Path) -> String {
    let script = dir.join("mark.sh");
    let m = marker.display().to_string().replace('\\', "/");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$0 $*\" >> '{m}'\nif [ -n \"$1\" ] && [ -f \"$1\" ]; then cat \"$1\"; else cat; fi\n"
        ),
    )
    .unwrap();
    format!("sh '{}'", script.display().to_string().replace('\\', "/"))
}

/// TM-16 completion: the repository configures an external diff, a
/// textconv and clean/smudge/process filters that leave a marker. Plain
/// `git diff` runs them (the active control); `review.diff`, `review.blob`
/// and `fs.write` (with its task touch) run none. The listing's revisions
/// are `fs.read`'s; a file changed after listing reads with a new revision;
/// a blob of another commit than the merge base is stale.
#[test]
fn the_review_diff_runs_no_repository_program() {
    let repo = Repo::new();
    let d = Daemon::start("files-diff");
    let mut c = d.client();
    let (t, wt) = task(&mut c, &repo);
    let marker = repo.root.join("marker.txt");
    let cmd = marker_script(&repo.root, &marker);
    for (k, v) in [
        ("diff.external", cmd.as_str()),
        ("diff.mark.textconv", cmd.as_str()),
        ("filter.mark.clean", cmd.as_str()),
        ("filter.mark.smudge", cmd.as_str()),
    ] {
        git_ok(&repo.path, &["config", k, v]);
    }
    std::fs::write(wt.join("src").join("main.rs"), "fn main() { changed(); }\n").unwrap();
    std::fs::remove_file(wt.join("gone.txt")).unwrap();
    std::fs::write(wt.join("untracked 새 파일.md"), "# hi\n").unwrap();

    // The active control: plain git runs the programs.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(&wt)
        .args(["diff", "HEAD"])
        .output()
        .unwrap();
    assert!(marker.exists(), "the control must run the marker");
    std::fs::remove_file(&marker).unwrap();
    git_ok(&repo.path, &["config", "filter.mark.process", &cmd]);

    let l = c
        .call::<ReviewDiff>(&ReviewDiffParams { task: t.clone() })
        .unwrap();
    let file = |p: &str| {
        l.files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("{p}: {l:?}"))
    };
    assert_eq!(file("src/main.rs").status, "M");
    assert_eq!(file("gone.txt").status, "D");
    assert_eq!(file("untracked 새 파일.md").status, "?");
    assert_eq!(file("untracked 새 파일.md").added, Some(1));
    assert_eq!(file("src/main.rs").attrs.text, "unspecified");
    let main = read(&mut c, &t, "src/main.rs").unwrap();
    assert_eq!(
        file("src/main.rs").revision.as_deref(),
        Some(main.revision.as_str())
    );
    let old = c
        .call::<ReviewBlob>(&ReviewBlobParams {
            task: t.clone(),
            rev: l.merge_base.clone(),
            path: "src/main.rs".into(),
        })
        .unwrap();
    assert_eq!(
        b64_decode(old.data.as_deref().unwrap()).unwrap(),
        b"fn main() {}\n"
    );
    let index = c
        .call::<ReviewBlob>(&ReviewBlobParams {
            task: t.clone(),
            rev: String::new(),
            path: "gone.txt".into(),
        })
        .unwrap();
    assert_eq!(
        b64_decode(index.data.as_deref().unwrap()).unwrap(),
        b"bye\n"
    );
    let none = c
        .call::<ReviewBlob>(&ReviewBlobParams {
            task: t.clone(),
            rev: l.merge_base.clone(),
            path: "untracked 새 파일.md".into(),
        })
        .unwrap();
    assert!(none.data.is_none());
    let w = write(
        &mut c,
        &t,
        "src/main.rs",
        b"fn main() { saved(); }\n",
        &main.revision,
        "op-diff-0001",
    )
    .unwrap();
    assert_eq!(w.outcome, FsWriteOutcome::Written);
    let again = c
        .call::<ReviewDiff>(&ReviewDiffParams { task: t.clone() })
        .unwrap();
    assert_ne!(
        again.snapshot, l.snapshot,
        "a saved file changes the snapshot"
    );
    assert!(
        !marker.exists(),
        "a repository program ran: {:?}",
        std::fs::read_to_string(&marker)
    );

    let e = rpc_err(c.call::<ReviewBlob>(&ReviewBlobParams {
        task: t.clone(),
        rev: "0".repeat(40),
        path: "src/main.rs".into(),
    }));
    assert_eq!(e.code, files_codes::STALE_DIFF);
    assert_eq!(e.data.unwrap()["merge_base"], json!(l.merge_base));
}
