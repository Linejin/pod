//! docs/17 R5 / docs/22 TM-16 over a bad network, on this PC: remote files
//! through `pod_remote::session` → loopback fault proxy → `pod
//! remote-stdio` → an isolated podd whose task worktree stands for the
//! remote machine's. A save whose answer is lost is sent again with the
//! same operation id and lands once; a save while disconnected keeps its
//! bytes; a stale save is a conflict; big files cross the remote line.
//!
//! Needs `podd.exe` and `pod-holder.exe` next to `pod.exe` (workspace build).

use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pod_remote::session::*;
use pod_test_support::fault_proxy::{FaultProxy, Upstream};
use pod_test_support::git::git_ok;
use podd::client::{Client, ProcessWatch};
use podd::files::{PendingSave, SaveOutcome, has_files, read_bytes, save_outcome};
use podd::protocol::*;
use podd::transport::{REMOTE_MAX_LINE, Transport};

const POD: &str = env!("CARGO_BIN_EXE_pod");
static SERIAL: Mutex<()> = Mutex::new(());

/// One isolated machine with a repository and a task `demo/files`.
struct Machine {
    root: PathBuf,
    pipe: String,
}

impl Machine {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("pod-r5-{tag}-{}-{nanos}", std::process::id());
        let root = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let podd = Path::new(POD).with_file_name("podd.exe");
        assert!(
            podd.exists() && podd.with_file_name("pod-holder.exe").exists(),
            "{} / pod-holder.exe missing: build the workspace first",
            podd.display()
        );
        Self {
            root,
            pipe: format!(r"\\.\pipe\{name}"),
        }
    }

    fn bridge(&self) -> std::io::Result<Child> {
        let r = &self.root;
        Command::new(POD)
            .args(["--pipe", &self.pipe, "remote-stdio"])
            .env("LOCALAPPDATA", r.join("local"))
            .env("APPDATA", r.join("roaming"))
            .env("USERPROFILE", r.join("home"))
            .env("POD_WORKTREES_ROOT", r.join("worktrees"))
            .env("POD_MODS_DIR", r.join("mods"))
            .env("POD_PR_POLL", "off")
            .env_remove("POD_SOCKET")
            .env_remove("POD_TERMINAL_ID")
            .env_remove("POD_DAEMON")
            .env_remove("POD_HOLDER")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    /// The task's worktree, created through a local client (the remote
    /// side's own tools).
    fn task(&self) -> (String, PathBuf) {
        let repo = self.root.join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git_ok(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("src").join("main.rs"), "fn main() {}\r\n").unwrap();
        std::fs::write(repo.join("한글 노트.md"), "\u{feff}# 제목\r\n").unwrap();
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "init"]);
        let mut c = Client::connect(&self.pipe).expect("local connect");
        c.call::<RepoAdd>(&RepoAddParams {
            path: repo.display().to_string(),
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
}

impl Drop for Machine {
    fn drop(&mut self) {
        if let Ok(mut c) = Client::connect(&self.pipe)
            && let Ok(s) = c.call::<ServerStatus>(&Empty {})
        {
            let watch = ProcessWatch::open(s.pid);
            let _ = c.call::<ServerStop>(&StopParams { kill: true });
            drop(c);
            if let Some(w) = watch {
                let _ = w.wait(Duration::from_secs(20));
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn proxy(m: Arc<Machine>) -> FaultProxy {
    FaultProxy::start(move || {
        let mut child = m.bridge()?;
        let reader = child.stdout.take().unwrap();
        let writer = child.stdin.take().unwrap();
        let child = Arc::new(Mutex::new(child));
        Ok(Upstream {
            reader: Box::new(reader),
            writer: Box::new(writer),
            close: Box::new(move || {
                let _ = child.lock().unwrap().kill();
            }),
        })
    })
    .unwrap()
}

struct ProxyConnector {
    addr: SocketAddr,
}

impl Connector for ProxyConnector {
    fn connect(&self) -> Result<Link, ConnectError> {
        let fail = |m: String| ConnectError {
            message: m,
            permanent: false,
        };
        let s = TcpStream::connect(self.addr).map_err(|e| fail(e.to_string()))?;
        let closer = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let r = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let mut c = Client::from_transport("proxy", Transport::new(r, s, REMOTE_MAX_LINE));
        match c.hello(&[capability::REMOTE_FILES]) {
            Ok(h) => Ok(Link::new(
                c,
                h,
                Arc::new(move || {
                    let _ = closer.shutdown(Shutdown::Both);
                }),
            )),
            Err(e) => Err(ConnectError {
                permanent: e.is_incompatible(),
                message: e.to_string(),
            }),
        }
    }
}

fn policy() -> ReconnectPolicy {
    ReconnectPolicy {
        first_delay: Duration::from_millis(300),
        max_delay: Duration::from_secs(2),
        jitter: 0.2,
        give_up_after: Some(Duration::from_secs(120)),
        call_timeout: Duration::from_secs(20),
    }
}

/// Machine, proxy, connected remote session and the task.
fn setup(tag: &str) -> (Arc<Machine>, FaultProxy, RemoteControl, String, PathBuf) {
    let m = Arc::new(Machine::new(tag));
    let px = proxy(m.clone());
    let conn: Arc<dyn Connector> = Arc::new(ProxyConnector { addr: px.addr() });
    let ctl = RemoteControl::connect(conn, policy()).unwrap();
    assert!(has_files(ctl.hello()), "{:?}", ctl.hello().capabilities);
    let (t, wt) = m.task();
    (m, px, ctl, t, wt)
}

fn read(ctl: &mut RemoteControl, t: &str, p: &str) -> FsReadResult {
    ctl.call::<FsRead>(&FsPathParams {
        task: t.into(),
        path: p.into(),
    })
    .unwrap()
}

fn send(ctl: &mut RemoteControl, s: &PendingSave) -> SaveOutcome {
    match ctl.call::<FsWrite>(s.params()) {
        Ok(r) => save_outcome(Ok(r)),
        Err(RemoteError::Rpc(e)) => save_outcome(Err(Some(&e))),
        Err(_) => save_outcome(Err(None)),
    }
}

fn reconnect(px: &FaultProxy, ctl: &mut RemoteControl) {
    while !px.is_cut() {
        std::thread::sleep(Duration::from_millis(50));
    }
    px.restore();
    ctl.reconnect(&AtomicBool::new(false)).unwrap();
}

/// The answer of a save is lost (the network drops right after the daemon
/// wrote): the outcome is unknown, the bytes are kept, and the same save
/// sent again after reconnecting is answered `already`: written once.
#[test]
fn a_save_whose_answer_is_lost_lands_once() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_m, px, mut ctl, t, wt) = setup("lost");
    let r = read(&mut ctl, &t, "한글 노트.md");
    let body = "\u{feff}# 제목\r\n본문\r\n".as_bytes();
    let save = PendingSave::new(&t, "한글 노트.md", body, &r.revision);

    px.drop_replies_then_cut(Duration::from_millis(1500));
    assert_eq!(send(&mut ctl, &save), SaveOutcome::Unknown);
    assert!(px.stats().dropped_bytes > 0, "the answer was swallowed");
    assert_eq!(
        std::fs::read(wt.join("한글 노트.md")).unwrap(),
        body,
        "it was written"
    );
    // Still down: nothing is sent, the save waits.
    reconnect(&px, &mut ctl);
    assert!(save.may_resend(std::time::SystemTime::now()));
    match send(&mut ctl, &save) {
        SaveOutcome::Saved { already, revision } => {
            assert!(already, "the resend is not a second write");
            assert_eq!(revision, read(&mut ctl, &t, "한글 노트.md").revision);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(wt.join("한글 노트.md")).unwrap(), body);
}

/// Disconnected before the save is sent: nothing reaches the daemon, the
/// save (and its bytes) wait, and once back it is written.
#[test]
fn a_save_while_disconnected_waits_and_then_lands() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_m, px, mut ctl, t, wt) = setup("down");
    let r = read(&mut ctl, &t, "src/main.rs");
    let save = PendingSave::new(&t, "src/main.rs", b"fn main() { 1; }\r\n", &r.revision);
    px.cut();
    // The session notices the drop on its next call.
    let first = send(&mut ctl, &save);
    assert_eq!(first, SaveOutcome::Unknown, "{first:?}");
    assert!(matches!(
        ctl.call::<FsRead>(&FsPathParams {
            task: t.clone(),
            path: "src/main.rs".into()
        }),
        Err(RemoteError::Disconnected { .. })
    ));
    assert_eq!(
        std::fs::read(wt.join("src").join("main.rs")).unwrap(),
        b"fn main() {}\r\n"
    );
    reconnect(&px, &mut ctl);
    assert!(matches!(
        send(&mut ctl, &save),
        SaveOutcome::Saved { already: false, .. }
    ));
    assert_eq!(
        std::fs::read(wt.join("src").join("main.rs")).unwrap(),
        b"fn main() { 1; }\r\n"
    );
}

/// The remote file changes after the client read it (an agent on that
/// machine): the save is a conflict with the new revision; nothing is
/// overwritten. The review diff lists the change with that revision.
#[test]
fn a_stale_remote_save_is_a_conflict() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_m, _px, mut ctl, t, wt) = setup("stale");
    let r = read(&mut ctl, &t, "src/main.rs");
    std::fs::write(wt.join("src").join("main.rs"), "fn agent() {}\r\n").unwrap();
    let save = PendingSave::new(&t, "src/main.rs", b"fn mine() {}\r\n", &r.revision);
    let now = read(&mut ctl, &t, "src/main.rs");
    assert_eq!(
        send(&mut ctl, &save),
        SaveOutcome::Conflict {
            current: Some(now.revision.clone())
        }
    );
    assert_eq!(read_bytes(&now).unwrap(), b"fn agent() {}\r\n");
    let l = ctl
        .call::<ReviewDiff>(&ReviewDiffParams { task: t.clone() })
        .unwrap();
    let f = l.files.iter().find(|f| f.path == "src/main.rs").unwrap();
    assert_eq!(f.revision.as_deref(), Some(now.revision.as_str()));
}

/// The largest file read (4 MiB) and write (3 MB) cross the remote line;
/// bigger ones are refused with TOO_LARGE and the session goes on.
#[test]
fn big_files_cross_the_remote_line_or_are_refused() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_m, _px, mut ctl, t, wt) = setup("big");
    std::fs::write(wt.join("edge.log"), vec![b'x'; MAX_READ_BYTES as usize]).unwrap();
    std::fs::write(wt.join("big.log"), vec![b'x'; MAX_READ_BYTES as usize + 1]).unwrap();
    let r = read(&mut ctl, &t, "edge.log");
    assert_eq!(read_bytes(&r).unwrap().len() as u64, MAX_READ_BYTES);
    match ctl.call::<FsRead>(&FsPathParams {
        task: t.clone(),
        path: "big.log".into(),
    }) {
        Err(RemoteError::Rpc(e)) => assert_eq!(e.code, files_codes::TOO_LARGE),
        other => panic!("{other:?}"),
    }
    let body = vec![b'y'; MAX_WRITE_BYTES as usize];
    let save = PendingSave::new(&t, "edge.log", &body, &r.revision);
    assert!(matches!(
        send(&mut ctl, &save),
        SaveOutcome::Saved { already: false, .. }
    ));
    let r2 = read(&mut ctl, &t, "edge.log");
    let more = vec![b'z'; MAX_WRITE_BYTES as usize + 1];
    let too_big = PendingSave::new(&t, "edge.log", &more, &r2.revision);
    assert!(matches!(
        send(&mut ctl, &too_big),
        SaveOutcome::Refused {
            code: files_codes::TOO_LARGE,
            ..
        }
    ));
    assert_eq!(std::fs::read(wt.join("edge.log")).unwrap(), body);
}
