//! `pod remote-stdio` against an isolated podd on this PC (docs/17 R2).
//!
//! Needs `podd.exe` and `pod-holder.exe` next to `pod.exe` (a workspace
//! build: `cargo test --workspace`, or `cargo build -p podd` first). Every
//! path the daemon would use (state, log, start lock, worktrees, mods) is
//! redirected to a temp directory through the environment.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use podd::client::{AttachEvent, Client, ClientError, ProcessWatch};
use podd::protocol::*;
use podd::transport::Transport;

const POD: &str = env!("CARGO_BIN_EXE_pod");

struct Env {
    root: PathBuf,
    pipe: String,
}

impl Env {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("pod-rstdio-{tag}-{}-{nanos}", std::process::id());
        let root = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&root).unwrap();
        let podd = Path::new(POD).with_file_name("podd.exe");
        assert!(
            podd.exists() && podd.with_file_name("pod-holder.exe").exists(),
            "{} / pod-holder.exe missing: build the workspace (cargo build -p podd) first",
            podd.display()
        );
        Self {
            root,
            pipe: format!(r"\\.\pipe\{name}"),
        }
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        let r = &self.root;
        c.env("LOCALAPPDATA", r.join("local"))
            .env("APPDATA", r.join("roaming"))
            .env("USERPROFILE", r.join("home"))
            .env("POD_WORKTREES_ROOT", r.join("worktrees"))
            .env("POD_MODS_DIR", r.join("mods"))
            .env("POD_PR_POLL", "off")
            .env_remove("POD_SOCKET")
            .env_remove("POD_TERMINAL_ID")
            .env_remove("POD_DAEMON")
            .env_remove("POD_HOLDER");
        let _ = std::fs::create_dir_all(r.join("home"));
        c
    }

    fn bridge_cmd(&self) -> Command {
        let mut c = self.command(POD);
        c.args(["--pipe", &self.pipe, "remote-stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    /// A bridge child and a client speaking over its stdio.
    fn bridge(&self) -> (Child, Client) {
        let mut child = self.bridge_cmd().spawn().expect("spawn pod remote-stdio");
        let t = Transport::from_child(&mut child).unwrap();
        (child, Client::from_transport("stdio:bridge", t))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("local").join("pod").join("podd.log"))
            .unwrap_or_default()
    }

    /// A direct pipe client (as the local app would connect).
    fn direct(&self) -> Client {
        Client::connect(&self.pipe).expect("direct connect")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Ok(mut c) = Client::connect(&self.pipe)
            && let Ok(s) = c.call::<ServerStatus>(&Empty {})
        {
            let watch = ProcessWatch::open(s.pid);
            let _ = c.call::<ServerStop>(&StopParams { kill: true });
            drop(c);
            if let Some(w) = watch {
                w.wait(Duration::from_secs(20));
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn wait_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        assert!(Instant::now() < deadline, "bridge did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stderr_of(child: &mut Child) -> String {
    let mut s = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut s);
    }
    s
}

fn running(c: &mut Client, id: &str) -> bool {
    c.call::<TerminalList>(&Empty {})
        .unwrap()
        .terminals
        .iter()
        .any(|t| t.id == id && t.state == TerminalState::Running)
}

#[test]
fn two_bridges_share_one_started_daemon_and_terminals_outlive_them() {
    let env = Env::new("two");
    // Both start before any daemon exists: the start lock lets one start it.
    let (mut b1, mut c1) = env.bridge();
    let (mut b2, mut c2) = env.bridge();
    let t1 = std::thread::spawn(move || {
        let h = c1.hello(&[]);
        (c1, h)
    });
    let h2 = c2.hello(&[]).expect("hello over bridge 2");
    let (mut c1, h1) = t1.join().unwrap();
    let h1 = h1.expect("hello over bridge 1");
    assert_eq!(h1.protocol, podd::protocol::PROTOCOL_VERSION);
    assert_eq!(h1.machine_id, h2.machine_id);
    assert_eq!(
        h1.boot_id, h2.boot_id,
        "both bridges reached the same daemon start"
    );
    let log = env.log();
    assert_eq!(
        log.matches("listening on").count(),
        1,
        "one daemon start:\n{log}"
    );
    assert!(
        !log.contains("already owns"),
        "a second podd was started:\n{log}"
    );

    // A terminal started through bridge 1, watched through bridge 2.
    let t = c1
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                "cmd".into(),
                "/d".into(),
                "/q".into(),
                "/k".into(),
                "prompt $G".into(),
            ],
            cwd: Some(env.root.display().to_string()),
            name: Some("remote".into()),
            ..Default::default()
        })
        .unwrap();
    let (_res, _snap, mut stream) = c2
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .unwrap();
    c1.call::<TerminalSend>(&SendParams {
        target: TerminalRef::id(&t.id),
        text: "echo over-the-bridge-원격".into(),
        enter: true,
        confirm: false,
    })
    .unwrap();
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline
        && String::from_utf8_lossy(&seen)
            .matches("over-the-bridge")
            .count()
            < 2
    {
        match stream.next_event().unwrap() {
            Some(AttachEvent::Output { data, .. }) => seen.extend(data),
            Some(_) => {}
            None => panic!("attach stream over the bridge closed"),
        }
    }
    assert!(
        String::from_utf8_lossy(&seen)
            .matches("over-the-bridge")
            .count()
            >= 2,
        "{}",
        String::from_utf8_lossy(&seen)
    );

    // Bridge 1: the client closes (stdin EOF) -> clean exit.
    drop(c1);
    assert!(wait_exit(&mut b1, Duration::from_secs(10)).success());
    // Bridge 2: dropped like a lost SSH session (killed).
    drop(stream);
    let _ = b2.kill();
    let _ = b2.wait();

    // The daemon, the holder and the program are still there.
    let mut d = env.direct();
    assert!(running(&mut d, &t.id), "terminal died with its bridges");
    let info = d
        .call::<TerminalList>(&Empty {})
        .unwrap()
        .terminals
        .into_iter()
        .find(|x| x.id == t.id)
        .unwrap();
    for pid in [info.pid, info.holder_pid].into_iter().flatten() {
        assert!(
            ProcessWatch::open(pid).is_some_and(|w| !w.wait(Duration::ZERO)),
            "pid {pid} gone"
        );
    }
    // A new bridge sees the same daemon start.
    let (mut b3, mut c3) = env.bridge();
    let h3 = c3.hello(&[]).unwrap();
    assert_eq!(h3.boot_id, h1.boot_id);
    assert!(running(&mut c3, &t.id));
    drop(c3);
    assert!(wait_exit(&mut b3, Duration::from_secs(10)).success());
    assert_eq!(stderr_of(&mut b3), "", "no diagnostics on a clean run");
}

#[test]
fn stdout_is_protocol_only_and_reaches_eof_although_the_daemon_lives_on() {
    let env = Env::new("stdout");
    let mut child = env.bridge_cmd().spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // Requests, then EOF: the answers still come (short drain grace).
    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server.status\"}\n\
              {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"connection.hello\",\"params\":{\"min_protocol\":98,\"max_protocol\":99}}\n\
              {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"connection.hello\",\"params\":{\"min_protocol\":12,\"max_protocol\":12}}\n",
        )
        .unwrap();
    drop(stdin);
    // read_to_end returns only when every holder of the pipe's write end is
    // gone; the daemon started by the bridge must not hold it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let out = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("stdout never reached EOF (inherited by the daemon?)");
    assert!(wait_exit(&mut child, Duration::from_secs(10)).success());
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("non-JSON stdout line {l:?}: {e}"))
        })
        .collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(
        lines[0]["result"]["protocol"],
        podd::protocol::PROTOCOL_VERSION
    );
    // Future-only client: refused with the daemon's range; nothing done.
    assert_eq!(lines[1]["error"]["code"], remote_codes::PROTOCOL_MISMATCH);
    assert_eq!(
        lines[1]["error"]["data"]["max_protocol"],
        podd::protocol::PROTOCOL_VERSION
    );
    assert_eq!(lines[2]["result"]["protocol"], 12);
    // The daemon still runs after the bridge is gone.
    assert_eq!(
        env.direct()
            .call::<ServerStatus>(&Empty {})
            .unwrap()
            .protocol,
        podd::protocol::PROTOCOL_VERSION
    );
}

#[test]
fn client_fails_closed_when_something_pollutes_the_remote_stdout() {
    let env = Env::new("pollute");
    // A login banner printed before the bridge, as a shell profile would.
    let mut child = env
        .command("cmd")
        .raw_arg(format!(
            "/d /c echo Welcome to the build host& {POD} --pipe {} remote-stdio",
            env.pipe
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Transport::from_child(&mut child).unwrap();
    let mut c = Client::from_transport("stdio:polluted", t);
    match c.hello(&[]) {
        Err(ClientError::Protocol(m)) => {
            assert!(m.contains("non-protocol output"), "{m}");
            assert!(m.contains("Welcome"), "{m}");
        }
        other => panic!("polluted stdout was accepted: {other:?}"),
    }
    drop(c);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn bridge_gives_up_when_the_daemon_does_not_start_in_time() {
    let env = Env::new("timeout");
    // A "daemon" that exits at once without opening the pipe (whoami
    // rejects `--pipe …`; its output goes to NUL).
    let fake = PathBuf::from(std::env::var("SystemRoot").unwrap())
        .join("System32")
        .join("whoami.exe");
    let start = Instant::now();
    let mut child = env
        .bridge_cmd()
        .env("POD_DAEMON", &fake)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(40));
    assert_eq!(status.code(), Some(3));
    assert!(
        start.elapsed() >= Duration::from_secs(9),
        "{:?}",
        start.elapsed()
    );
    let err = stderr_of(&mut child);
    assert!(err.contains("did not open"), "{err}");
    assert!(err.lines().count() <= 16 && err.len() < 16 * 400, "{err}");
    let mut out = Vec::new();
    stdout.read_to_end(&mut out).unwrap();
    assert!(out.is_empty(), "stdout must stay protocol-only: {out:?}");
}
