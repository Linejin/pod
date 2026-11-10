//! daemon-security-02 end to end: the holder's own replies to terminal
//! queries are bounded. The child never reads its input (simulated with a
//! debug-only knob that stalls the holder's PTY writer) and keeps writing
//! OSC 7501 queries; the holder must drop replies past its budget, log it,
//! and stay responsive (kill still lands).

mod common;

use std::time::{Duration, Instant};

use common::*;
use podd::protocol::*;

fn dropped_logged(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .any(|e| {
            std::fs::read_to_string(e.path())
                .is_ok_and(|s| s.contains("terminal query replies dropped"))
        })
}

#[test]
fn query_flood_from_a_child_that_never_reads_is_bounded() {
    let d = Daemon::start_env("reply-budget", &[("POD_HOLDER_TEST_STALL_INPUT", "1")]);
    let mut c = d.client();
    // 100 queries per write, forever; never reads stdin.
    let script = "$q = ([string][char]27 + ']7501;?' + [char]7) * 100; \
                  while ($true) { [Console]::Write($q); Start-Sleep -Milliseconds 5 }";
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                "powershell".into(),
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-Command".into(),
                script.into(),
            ],
            cwd: cwd(),
            name: Some("asker".into()),
            ..Default::default()
        })
        .expect("run");

    let deadline = Instant::now() + Duration::from_secs(60);
    while !dropped_logged(&d.state_dir) {
        assert!(
            Instant::now() < deadline,
            "the holder never dropped a reply: replies are unbounded"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // Still responsive while the child keeps asking.
    let list = c.call::<TerminalList>(&Empty {}).expect("list").terminals;
    assert!(list.iter().any(|x| x.id == t.id), "{list:?}");
    let t0 = Instant::now();
    let info = c
        .call::<TerminalKill>(&TerminalRef::parse(&t.id))
        .expect("kill");
    assert_eq!(info.state, TerminalState::Exited, "{info:?}");
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "kill took {:?}",
        t0.elapsed()
    );
}
