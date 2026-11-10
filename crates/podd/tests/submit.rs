//! B1 (docs/12): typed/pasted text must be submitted by its Enter in TUIs
//! that detect pastes by timing (Codex on Windows). The `burst-loop` mode
//! of `fake-agent` reads console key events through the bundled ConPTY and
//! applies Codex's paste-burst rules.

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::*;
use podd::client::Client;
use podd::protocol::*;

fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Same single cargo invocation as the other integration tests.
fn workspace_bin(rel: &str) -> PathBuf {
    static BUILT: Mutex<bool> = Mutex::new(false);
    let mut built = BUILT.lock().unwrap();
    let dir = profile_dir();
    if !*built {
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args([
            "build",
            "-p",
            "podd",
            "--example",
            "fake-agent",
            "-p",
            "pod-hook",
            "-p",
            "pod-cli",
            "--bin",
            "pod",
            "--bin",
            "pod-hook",
        ]);
        if dir.file_name().and_then(|n| n.to_str()) == Some("release") {
            cmd.arg("--release");
        }
        assert!(cmd.status().expect("cargo build").success());
        *built = true;
    }
    let p = dir.join(rel);
    assert!(p.exists(), "{} missing", p.display());
    p
}

fn burst_agent(c: &mut Client, log: &std::path::Path, cost_us: u64, pre: &[&str]) -> String {
    let mut argv = vec![
        workspace_bin("examples/fake-agent.exe")
            .display()
            .to_string(),
    ];
    argv.extend(pre.iter().map(|s| s.to_string()));
    argv.push(format!("burst-loop:{}:{cost_us}", log.display()));
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv,
            cwd: cwd(),
            name: None,
            ..Default::default()
        })
        .expect("run burst-loop");
    wait_output(c, &t.id, "ready for prompts");
    t.id
}

fn submissions(log: &std::path::Path) -> Vec<String> {
    let s = std::fs::read_to_string(log).unwrap_or_default();
    s.split_terminator("<<<END")
        .map(|x| x.split_once(">>>\n").map_or(x, |(_, r)| r).to_string())
        .map(|x| x.trim_end().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn wait_for<T>(secs: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn long_prompt(n: usize) -> String {
    (1..=n)
        .map(|i| format!("line {i}: please address this review comment — keep it short."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn send_text(c: &mut Client, t: &str, text: &str) -> SendResult {
    c.call::<TerminalSend>(&SendParams {
        target: TerminalRef::parse(t),
        text: text.into(),
        enter: true,
        confirm: true,
    })
    .expect("terminal.send")
}

fn log_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(unique(tag))
}

/// `pod send` used to press Enter 30 ms after the text: inside the
/// 120 ms paste-burst window, so Codex inserted a newline and the text
/// stayed in its input box (docs/12 B1). Now Enter waits until the TUI has
/// drawn the text and gone quiet, and the agent's turn confirms it.
#[test]
fn send_submits_to_a_paste_burst_tui() {
    let d = Daemon::start("submit-send");
    let mut c = d.client();
    let log = log_path("burst-send");
    let t = burst_agent(&mut c, &log, 0, &[]);
    let text = "Run this command and wait for its result: powershell -NoProfile -File C:/x/gate-flow.ps1 -Pod C:/x/pod.exe . It requests a Pod gate; the human will approve in the app. Then report the result briefly. Do not delegate.";
    let r = send_text(&mut c, &t, text);
    let s = r.submit.expect("submit check");
    assert_eq!(s.state, SubmitState::Confirmed, "{s:?}");
    assert_eq!(s.enters, 1, "{s:?}");
    assert_eq!(submissions(&log), [text]);
    // A second message after the turn: again exactly one submission.
    wait_for(10, || {
        (read(&mut c, &t, ReadMode::Screen)
            .join("\n")
            .matches("done")
            .count()
            >= 1)
            .then_some(())
    })
    .expect("turn ended");
    let r = send_text(&mut c, &t, "second message");
    assert_eq!(r.submit.unwrap().state, SubmitState::Confirmed);
    assert_eq!(submissions(&log), [text, "second message"]);
    let _ = std::fs::remove_file(&log);
}

/// A TUI that lags behind a long burst (0.3 ms per key: ~0.6 s for this
/// text): the old fixed pause pressed Enter while it was still reading the
/// burst. Enter now waits for the burst to be drawn.
#[test]
fn send_waits_for_a_lagging_tui() {
    let d = Daemon::start("submit-lag");
    let mut c = d.client();
    let log = log_path("burst-lag");
    let t = burst_agent(&mut c, &log, 300, &[]);
    // Typed line breaks are Enter keys inside the burst: newlines.
    let text = long_prompt(30);
    let r = send_text(&mut c, &t, &text.replace('\n', "\r"));
    let s = r.submit.expect("submit check");
    assert_eq!(s.state, SubmitState::Confirmed, "{s:?}");
    // Normally one Enter; under heavy CPU load the console host can hand
    // the burst over in chunks further apart than the quiet window, and the
    // retry Enter submits it. Either way: exactly one, intact submission.
    assert!(s.enters <= 2, "{s:?}");
    assert_eq!(submissions(&log), [text]);
    let _ = std::fs::remove_file(&log);
}

/// An Enter the agent missed is retried once; an agent that never reacts
/// is reported as unconfirmed (not as submitted).
#[test]
fn missed_enter_is_retried_once_then_reported() {
    let d = Daemon::start("submit-retry");
    let mut c = d.client();
    let log = log_path("burst-retry");
    let t = burst_agent(&mut c, &log, 0, &["swallow-enters:1"]);
    let r = send_text(&mut c, &t, "please submit me");
    let s = r.submit.expect("submit check");
    assert_eq!((s.state, s.enters), (SubmitState::Confirmed, 2), "{s:?}");
    assert_eq!(s.signal.as_deref(), Some("working"));
    assert_eq!(submissions(&log), ["please submit me"]);

    let log2 = log_path("burst-deaf");
    let t2 = burst_agent(&mut c, &log2, 0, &["swallow-enters:9"]);
    let started = Instant::now();
    let r = send_text(&mut c, &t2, "nobody hears me");
    let s = r.submit.expect("submit check");
    assert_eq!((s.state, s.enters), (SubmitState::Unconfirmed, 2), "{s:?}");
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(submissions(&log2).is_empty());
    // --no-confirm: one Enter, no check.
    let r = c
        .call::<TerminalSend>(&SendParams {
            target: TerminalRef::parse(&t2),
            text: "x".into(),
            enter: true,
            confirm: false,
        })
        .unwrap();
    assert_eq!(r.submit.unwrap().state, SubmitState::Unverifiable);
    for l in [log, log2] {
        let _ = std::fs::remove_file(l);
    }
}

/// Shells report no agent status: one Enter, `unverifiable`, no retry.
#[test]
fn shell_send_is_unverifiable() {
    let d = Daemon::start("submit-shell");
    let mut c = d.client();
    let ps = powershell(&mut c, "sh");
    wait_output(&mut c, &ps.id, "PS ");
    let r = send_text(&mut c, &ps.id, "echo ('sub'+'mit-ok')");
    let s = r.submit.expect("submit check");
    assert_eq!((s.state, s.enters), (SubmitState::Unverifiable, 1), "{s:?}");
    wait_output(&mut c, &ps.id, "submit-ok");
}
