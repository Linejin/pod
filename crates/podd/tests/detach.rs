//! Auto-start must not leak the caller's handles into podd (docs/09 §5-10):
//! `pod ls | ...` with no daemon running starts podd, and the reader of
//! pod's stdout must still see EOF as soon as `pod` exits.

mod common;

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::pod_exe;
use podd::client::Client;
use podd::protocol::*;

/// Put this test process (and so `pod` and the daemon it starts) in a Job
/// Object that does not allow breakaway, like the job of a terminal or agent
/// that runs `pod`. That is the path where the old `Command`-based spawn
/// leaked the caller's pipes (with breakaway allowed it did not show).
fn enter_job_without_breakaway() {
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: plain Win32 calls; the job handle is intentionally leaked so
    // the job lives as long as this test process.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null(), "CreateJobObjectW");
        assert_ne!(
            AssignProcessToJobObject(job, GetCurrentProcess()),
            0,
            "AssignProcessToJobObject: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[test]
fn piped_pod_ls_reaches_eof_while_the_auto_started_daemon_runs() {
    let pod = pod_exe();
    enter_job_without_breakaway();
    let name = format!("podd-test-detach-{}-{}", std::process::id(), podd::now_ms());
    let pipe = format!(r"\\.\pipe\{name}");
    assert!(
        matches!(
            Client::connect(&pipe),
            Err(podd::client::ClientError::NotRunning)
        ),
        "no daemon must run yet"
    );

    let start = Instant::now();
    let mut child = Command::new(&pod)
        .arg("--pipe")
        .arg(&pipe)
        .arg("ls")
        .env("POD_DAEMON", env!("CARGO_BIN_EXE_podd"))
        .env("POD_HOLDER", env!("CARGO_BIN_EXE_pod-holder"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pod");
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        let _ = tx.send(("stdout", b));
    });
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        let _ = tx2.send(("stderr", b));
    });
    let mut got = Vec::new();
    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(x) => got.push(x),
            Err(_) => {
                let _ = child.kill();
                // Stop the leaked daemon, or it keeps cargo's pipes open too.
                if let Ok(mut c) = Client::connect(&pipe) {
                    let _ = c.call::<ServerStop>(&StopParams { kill: true });
                }
                panic!(
                    "pod's output pipes never reached EOF after {:?} (the daemon inherited them)",
                    start.elapsed()
                );
            }
        }
    }
    let status = child.wait().unwrap();
    let eof_after = start.elapsed();
    let stdout = got
        .iter()
        .find(|(k, _)| *k == "stdout")
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    assert!(status.success(), "pod ls failed: {got:?}");

    // The daemon pod started is still alive: EOF did not come from its exit.
    let mut c = Client::connect(&pipe).expect("auto-started daemon is running");
    let st = c.call::<ServerStatus>(&Empty {}).unwrap();
    assert_eq!(st.terminals, 0, "{stdout}");
    let state_dir = PathBuf::from(&st.state_dir);
    let _ = c.call::<ServerStop>(&StopParams { kill: true });
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Client::connect(&pipe).is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&state_dir);
    assert!(
        eof_after < Duration::from_secs(15),
        "EOF took {eof_after:?}"
    );
}
