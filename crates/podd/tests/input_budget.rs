//! daemon-01 end to end: input to a holder is bounded by acknowledgements.
//!
//! The daemon keeps at most 4 MiB of input in flight per terminal; the holder
//! (protocol 3) acknowledges what it wrote to the PTY. More than the budget
//! must therefore get through to a program that reads its input (refusals
//! are only temporary), and a kill is never stuck behind queued input.

mod common;

use std::time::{Duration, Instant};

use base64::Engine;
use common::*;
use podd::client::ClientError;
use podd::protocol::*;

#[test]
fn more_than_the_budget_reaches_a_reading_program_and_kill_still_lands() {
    let d = Daemon::start("input-budget");
    let mut c = d.client();
    // Reads console input line by line and prints nothing.
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["findstr".into(), "/x".into(), "NEVER-MATCHES-0xBEEF".into()],
            cwd: cwd(),
            name: Some("reader".into()),
            ..Default::default()
        })
        .expect("run findstr");

    let mut line = vec![b'x'; 1023];
    line.push(b'\r');
    let chunk: Vec<u8> = line.repeat(64); // 64 KiB per request
    let data = base64::engine::general_purpose::STANDARD.encode(&chunk);
    let total = 6 * 1024 * 1024; // past the 4 MiB budget
    let (mut sent, mut refused) = (0usize, 0usize);
    let deadline = Instant::now() + Duration::from_secs(120);
    while sent < total {
        assert!(
            Instant::now() < deadline,
            "only {sent} of {total} bytes accepted ({refused} refusals): input is never acknowledged"
        );
        match c.call::<TerminalInput>(&InputParams {
            target: TerminalRef::parse(&t.id),
            data: data.clone(),
        }) {
            Ok(_) => sent += chunk.len(),
            Err(ClientError::Rpc(e)) if e.code == codes::INPUT_BACKLOG => {
                refused += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("terminal.input: {e}"),
        }
    }
    eprintln!("sent {sent} bytes, {refused} temporary refusals");

    let t0 = Instant::now();
    let info = c
        .call::<TerminalKill>(&TerminalRef::parse(&t.id))
        .expect("kill");
    assert_eq!(info.state, TerminalState::Exited, "{info:?}");
    assert!(info.killed);
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "kill took {:?}",
        t0.elapsed()
    );
}
