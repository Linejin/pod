//! Holder contract (refactor card D-P4, docs/20-refactor/daemon.md): what
//! `podd::holder` keeps while its budgets, input queue, process and daemon
//! connection live in their own modules.
//!
//! - the facade: the public constants and `holder::main`;
//! - the binary frame golden: every frame kind, byte for byte;
//! - a real `pod-holder` driven over its pipe with no daemon: record, hello
//!   and snapshot, output `seq` continuity, protocol-3 input acks, resize
//!   clamping, a release that waits for the child's exit, the exit replayed
//!   to a reconnecting daemon, and control frames (resize, kill) that land
//!   while the output budget holds the child back.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use podd::holder;
use podd::holder_proto::{
    DaemonHello, ExitInfo, Frame, HOLDER_PROTOCOL, HOLDER_PROTOCOL_MIN, HolderHello, HolderRecord,
    HolderSpec, read_frame,
};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

use common::{process_alive, unique, until};

#[test]
fn facade_constants_and_entry_point() {
    assert_eq!(holder::OUTPUT_CHUNK, 64 * 1024);
    assert_eq!(holder::OUTPUT_BUDGET, 8 * 1024 * 1024);
    assert_eq!(holder::INPUT_HARD_CAP, 16 * 1024 * 1024);
    assert_eq!(holder::REPLY_BYTE_CAP, 256 * 1024);
    assert_eq!(holder::REPLY_COUNT_CAP, 4096);
    assert_eq!(holder::HOLDER_PROTOCOL, 3);
    assert_eq!(holder::HOLDER_PROTOCOL, HOLDER_PROTOCOL);
    assert_eq!(HOLDER_PROTOCOL_MIN, 1);
    let _entry: fn() -> i32 = holder::main;
}

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// `u32 len` (kind + body), `u8 kind`, JSON body.
fn json_frame(kind: u8, body: &str) -> Vec<u8> {
    let mut v = ((body.len() + 1) as u32).to_le_bytes().to_vec();
    v.push(kind);
    v.extend(body.as_bytes());
    v
}

#[tokio::test]
async fn frame_golden() {
    let hello = HolderHello {
        version: 3,
        id: "t1".into(),
        name: Some("n".into()),
        argv: vec!["cmd".into()],
        cwd: "C:\\".into(),
        created_ms: 5,
        holder_pid: 1,
        child_pid: 2,
        cols: 80,
        rows: 24,
        conpty: "inbox".into(),
        parent_terminal: Some("t0".into()),
        parent_task: None,
    };
    let cases: Vec<(Frame, Vec<u8>)> = vec![
        (
            Frame::Hello(hello),
            json_frame(
                0x01,
                r#"{"version":3,"id":"t1","name":"n","argv":["cmd"],"cwd":"C:\\","created_ms":5,"holder_pid":1,"child_pid":2,"cols":80,"rows":24,"conpty":"inbox","parent_terminal":"t0"}"#,
            ),
        ),
        (
            Frame::Snapshot {
                seq: 7,
                data: b"hi".to_vec(),
            },
            hex("0b000000 02 0700000000000000 6869"),
        ),
        (
            Frame::Output {
                seq: 0x0102_0304_0506_0708,
                data: vec![0, 255, 10],
            },
            hex("0c000000 03 0807060504030201 00ff0a"),
        ),
        (
            Frame::Resized {
                seq: 12,
                cols: 100,
                rows: 40,
            },
            hex("0d000000 04 0c00000000000000 6400 2800"),
        ),
        (
            Frame::Exited(ExitInfo {
                seq: 12,
                exit_code: Some(3),
                killed: true,
                exited_ms: 9,
            }),
            json_frame(
                0x05,
                r#"{"seq":12,"exit_code":3,"killed":true,"exited_ms":9}"#,
            ),
        ),
        (
            Frame::Exited(ExitInfo {
                seq: 0,
                exit_code: None,
                killed: false,
                exited_ms: 1,
            }),
            json_frame(
                0x05,
                r#"{"seq":0,"exit_code":null,"killed":false,"exited_ms":1}"#,
            ),
        ),
        (
            Frame::InputAck { written: 1 << 40 },
            hex("09000000 06 0000000000010000"),
        ),
        (Frame::Input(b"ls\r".to_vec()), hex("04000000 81 6c730d")),
        (
            Frame::Resize {
                cols: 120,
                rows: 30,
            },
            hex("05000000 82 7800 1e00"),
        ),
        (Frame::Kill, hex("01000000 83")),
        (Frame::Release, hex("01000000 84")),
        (
            Frame::DaemonHello(DaemonHello { version: 3 }),
            hex("0e000000 85 7b2276657273696f6e223a337d"),
        ),
    ];
    let mut stream = Vec::new();
    for (frame, golden) in &cases {
        assert_eq!(&frame.encode(), golden, "{frame:?}");
        let back = read_frame(&mut &golden[..]).await.unwrap();
        assert_eq!(back.as_ref(), Some(frame));
        stream.extend(golden);
    }
    let mut r = &stream[..];
    for (frame, _) in &cases {
        assert_eq!(read_frame(&mut r).await.unwrap().as_ref(), Some(frame));
    }
    assert_eq!(read_frame(&mut r).await.unwrap(), None);
    // Framing errors.
    let e = read_frame(&mut &hex("00000000")[..]).await.unwrap_err();
    assert_eq!(e.to_string(), "bad holder frame length 0");
    let e = read_frame(&mut &hex("01000000 7f")[..]).await.unwrap_err();
    assert_eq!(e.to_string(), "bad holder frame: unknown kind 0x7f");
    let e = read_frame(&mut &hex("05000000 03 0102")[..])
        .await
        .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
}

/// A `pod-holder` started from a spec file, as podd starts it, in its own
/// temp directory on a test-only pipe. Dropping it kills the holder (and so,
/// through the Job Object, the child) and removes the directory.
struct Holder {
    child: Child,
    dir: PathBuf,
    pipe: String,
    record: PathBuf,
}

impl Holder {
    fn start(tag: &str, argv: &[&str], cols: u16, rows: u16) -> Self {
        let name = unique(tag);
        let dir = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        let pipe = format!(r"\\.\pipe\{name}");
        let record = dir.join("t1.json");
        let spec = HolderSpec {
            id: "t1".into(),
            name: Some("contract".into()),
            argv: argv.iter().map(|a| a.to_string()).collect(),
            cwd: dir.display().to_string(),
            env: std::env::vars().collect(),
            cols,
            rows,
            pipe: pipe.clone(),
            record_path: record.display().to_string(),
            created_ms: 5,
            parent_terminal: None,
            parent_task: None,
        };
        let spec_path = dir.join("spec.json");
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_pod-holder"))
            .arg("--spec")
            .arg(&spec_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pod-holder");
        let h = Self {
            child,
            dir,
            pipe,
            record,
        };
        let err = h.record.with_extension("err");
        until("the holder record", 30, || {
            if let Ok(e) = std::fs::read_to_string(&err) {
                panic!("pod-holder failed: {e}");
            }
            h.record.exists().then_some(())
        });
        assert!(!spec_path.exists(), "the holder removes its spec");
        h
    }

    fn record(&self) -> HolderRecord {
        serde_json::from_slice(&std::fs::read(&self.record).unwrap()).unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.record.with_extension("log")).unwrap_or_default()
    }

    async fn connect(&self) -> Conn {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match ClientOptions::new().open(&self.pipe) {
                Ok(c) => {
                    let (rd, wr) = tokio::io::split(c);
                    return Conn {
                        rd,
                        wr,
                        seq: 0,
                        text: Vec::new(),
                    };
                }
                Err(e) if Instant::now() > deadline => panic!("connect {}: {e}", self.pipe),
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One daemon-side connection. Tracks the stream position: every live frame
/// must carry the `seq` where the previous one ended.
struct Conn {
    rd: ReadHalf<NamedPipeClient>,
    wr: WriteHalf<NamedPipeClient>,
    seq: u64,
    text: Vec<u8>,
}

impl Conn {
    async fn send(&mut self, f: Frame) {
        self.wr.write_all(&f.encode()).await.expect("write frame");
    }

    async fn next(&mut self) -> Frame {
        let f = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut self.rd))
            .await
            .expect("a frame within 30 s")
            .expect("read frame")
            .expect("the holder closed the connection");
        match &f {
            Frame::Snapshot { seq, .. } => self.seq = *seq,
            Frame::Output { seq, data } => {
                assert_eq!(*seq, self.seq, "output continues the stream");
                self.seq += data.len() as u64;
                self.text.extend(data);
            }
            Frame::Resized { seq, .. } => assert_eq!(*seq, self.seq, "resized in stream order"),
            Frame::Exited(e) => assert_eq!(e.seq, self.seq, "exit at the end of the stream"),
            _ => {}
        }
        f
    }

    /// Hello, then the snapshot.
    async fn handshake(&mut self) -> HolderHello {
        let Frame::Hello(hello) = self.next().await else {
            panic!("hello first");
        };
        assert!(
            matches!(self.next().await, Frame::Snapshot { .. }),
            "snapshot second"
        );
        hello
    }

    /// Frames until `f` picks one (output on the way is kept in `text`).
    async fn until<T>(&mut self, what: &str, mut f: impl FnMut(&Frame) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(Instant::now() < deadline, "no {what} within 60 s");
            let frame = self.next().await;
            if let Some(t) = f(&frame) {
                return t;
            }
            assert!(
                !matches!(frame, Frame::Exited(_)),
                "exited before {what}: {frame:?}"
            );
        }
    }

    async fn until_text(&mut self, needle: &str) {
        while !String::from_utf8_lossy(&self.text).contains(needle) {
            let f = self.next().await;
            assert!(!matches!(f, Frame::Exited(_)), "exited before {needle:?}");
        }
    }
}

fn wait_exit(child: &mut Child, secs: u64) -> std::process::ExitStatus {
    until("the holder process to exit", secs, || {
        child.try_wait().unwrap()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holder_lifecycle_over_its_pipe() {
    let mut h = Holder::start("holder-life", &["cmd.exe", "/d", "/q"], 80, 24);
    let rec = h.record();
    assert_eq!(rec.id, "t1");
    assert_eq!(rec.name.as_deref(), Some("contract"));
    assert_eq!(rec.pipe, h.pipe);
    assert_eq!(rec.holder_pid, h.child.id());
    assert_eq!(rec.created_ms, 5);
    assert!(process_alive(rec.child_pid));

    let mut c = h.connect().await;
    let hello = c.handshake().await;
    assert_eq!(hello.version, HOLDER_PROTOCOL);
    assert_eq!(hello.id, "t1");
    assert_eq!(hello.name.as_deref(), Some("contract"));
    assert_eq!(hello.argv, ["cmd.exe", "/d", "/q"]);
    assert_eq!(hello.cwd, h.dir.display().to_string());
    assert_eq!(hello.created_ms, 5);
    assert_eq!(hello.holder_pid, rec.holder_pid);
    assert_eq!(hello.child_pid, rec.child_pid);
    assert_eq!((hello.cols, hello.rows), (80, 24));
    assert!(
        hello.conpty == "inbox" || hello.conpty.starts_with("bundled:"),
        "{}",
        hello.conpty
    );
    assert_eq!((hello.parent_terminal, hello.parent_task), (None, None));

    // Protocol 3: input is acknowledged once written, counted per connection.
    c.send(Frame::DaemonHello(DaemonHello { version: 3 })).await;
    let line = b"set /a 1300+37\r";
    c.send(Frame::Input(line.to_vec())).await;
    let written = c
        .until("input ack", |f| match f {
            Frame::InputAck { written } => Some(*written),
            _ => None,
        })
        .await;
    assert_eq!(written, line.len() as u64);
    c.until_text("1337").await;

    // Resize: clamped to 2..=1000 x 2..=500, answered in stream order.
    c.send(Frame::Resize {
        cols: 1,
        rows: 9999,
    })
    .await;
    let size = c
        .until("resized", |f| match f {
            Frame::Resized { cols, rows, .. } => Some((*cols, *rows)),
            _ => None,
        })
        .await;
    assert_eq!(size, (2, 500));

    // Release while the child runs is ignored: the holder still answers.
    c.send(Frame::Release).await;
    c.send(Frame::Resize {
        cols: 100,
        rows: 30,
    })
    .await;
    let size = c
        .until("resized after release", |f| match f {
            Frame::Resized { cols, rows, .. } => Some((*cols, *rows)),
            _ => None,
        })
        .await;
    assert_eq!(size, (100, 30));
    assert!(h.child.try_wait().unwrap().is_none(), "release ignored");

    // The child exits: one Exited at the end of the stream.
    c.send(Frame::Input(b"exit\r".to_vec())).await;
    let exit = c
        .until("exited", |f| match f {
            Frame::Exited(e) => Some(e.clone()),
            _ => None,
        })
        .await;
    assert_eq!((exit.exit_code, exit.killed), (Some(0), false));
    let end = c.seq;

    // A reconnecting daemon gets hello, a snapshot where the stream ended and
    // the same exit.
    let mut c2 = h.connect().await;
    let hello2 = c2.handshake().await;
    assert_eq!((hello2.cols, hello2.rows), (100, 30));
    assert_eq!(hello2.child_pid, rec.child_pid);
    assert_eq!(c2.seq, end);
    assert_eq!(c2.next().await, Frame::Exited(exit));

    // Release after the exit: the holder removes its record and log, exits 0.
    c2.send(Frame::Release).await;
    let status = wait_exit(&mut h.child, 20);
    assert_eq!(status.code(), Some(0));
    assert!(!h.record.exists(), "record removed on release");
    assert!(
        !h.record.with_extension("log").exists(),
        "log removed on release"
    );
}

/// Bytes the child has written (it counts them in a file).
fn written_so_far(count: &Path) -> u64 {
    std::fs::read_to_string(count)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_frames_land_while_output_is_held_back() {
    let name = unique("holder-flood-count");
    let count = std::env::temp_dir().join(format!("{name}.txt"));
    let script = format!(
        "$f = '{}'; $s = 'x' * 32000; $n = 0; \
         while ($true) {{ [Console]::Out.Write($s); $n += $s.Length; \
         [IO.File]::WriteAllText($f, [string]$n) }}",
        count.display()
    );
    let h = Holder::start(
        "holder-flood",
        &["powershell", "-NoLogo", "-NoProfile", "-Command", &script],
        400,
        200,
    );
    let mut c = h.connect().await;
    let hello = c.handshake().await;
    c.send(Frame::DaemonHello(DaemonHello { version: 3 })).await;

    // Read nothing: the holder queues output up to its budget, then stops
    // reading the PTY, and the child blocks on its own output.
    let deadline = Instant::now() + Duration::from_secs(120);
    let (mut last, mut since) = (0, Instant::now());
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let n = written_so_far(&count);
        if n != last {
            (last, since) = (n, Instant::now());
        } else if n > 0 && since.elapsed() > Duration::from_secs(8) {
            // 8 s without progress, not 3: on a loaded machine (parallel
            // builds) a CPU-starved child can pause for seconds before it
            // really blocks on its output. ConPTY re-renders, so the child's
            // own count cannot be compared with the budget.
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the child never blocked on its output ({last} bytes written)"
        );
    }

    // Control frames still land: the resize is answered in stream order and
    // the kill ends the child while the output is still held back.
    c.send(Frame::Resize {
        cols: 100,
        rows: 30,
    })
    .await;
    c.send(Frame::Kill).await;
    until("the killed child to exit", 20, || {
        (!process_alive(hello.child_pid)).then_some(())
    });
    let size = c
        .until("resized", |f| match f {
            Frame::Resized { cols, rows, .. } => Some((*cols, *rows)),
            _ => None,
        })
        .await;
    assert_eq!(size, (100, 30));
    // Everything before the resize was queued when it landed: about the
    // budget (frame headers count against it too).
    let held_back = c.text.len();
    assert!(
        // Frame headers count against the budget and ConPTY often writes
        // chunks smaller than OUTPUT_CHUNK, so "about the budget" is 90 %.
        held_back > holder::OUTPUT_BUDGET / 10 * 9,
        "{held_back} bytes were queued when the resize landed"
    );
    let exit = loop {
        if let Frame::Exited(e) = c.next().await {
            break e;
        }
    };
    assert!(exit.killed, "{exit:?}");
    assert_eq!(exit.exit_code, Some(1));
    assert!(h.log().contains("kill (job)"), "{}", h.log());
    let _ = std::fs::remove_file(&count);
}
