//! Debug tool: attach to a terminal and print its raw output stream, escaped,
//! with timestamps, for a few seconds. Never sends input.
//!
//! ```text
//! cargo run --example tap -- <terminal> [seconds] [--pipe PIPE] [--only-osc]
//! ```
//! `--only-osc` prints just the OSC sequences (e.g. `ESC]7501;state=idle...`).

use std::time::{Duration, Instant};

use podd::client::{AttachEvent, Client};
use podd::endpoint;
use podd::protocol::{AttachParams, TerminalRef};

fn escape(bytes: &[u8]) -> String {
    bytes.escape_ascii().to_string()
}

/// Complete OSC sequences in `buf` (ESC ] ... BEL | ESC \), draining what was consumed.
fn take_oscs(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut keep_from = buf.len().saturating_sub(4096);
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == 0x1b && buf[i + 1] == b']' {
            let end = (i + 2..buf.len()).find_map(|j| match buf[j] {
                0x07 => Some(j + 1),
                0x1b if buf.get(j + 1) == Some(&b'\\') => Some(j + 2),
                _ => None,
            });
            match end {
                Some(e) => {
                    out.push(buf[i..e].to_vec());
                    i = e;
                    keep_from = e;
                    continue;
                }
                None => {
                    keep_from = i; // incomplete: keep for the next chunk
                    break;
                }
            }
        }
        i += 1;
    }
    buf.drain(..keep_from.min(buf.len()));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let terminal = args
        .first()
        .expect("usage: tap <terminal> [seconds] [--pipe PIPE] [--only-osc]")
        .clone();
    let secs: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let pipe = args
        .iter()
        .position(|a| a == "--pipe")
        .and_then(|i| args.get(i + 1))
        .map(|p| endpoint::pipe_path(p))
        .unwrap_or_else(endpoint::default_pipe);
    let only_osc = args.iter().any(|a| a == "--only-osc");

    let (res, snapshot, mut stream) = Client::connect(&pipe)
        .expect("connect")
        .attach(&AttachParams {
            target: TerminalRef::parse(&terminal),
            scrollback: Some(0),
            ..Default::default()
        })
        .expect("attach");
    println!(
        "# attached to {} at seq {} ({} snapshot bytes)",
        res.terminal.id,
        res.seq,
        snapshot.len()
    );
    let start = Instant::now();
    let deadline = start + Duration::from_secs_f64(secs);
    std::thread::spawn(move || {
        std::thread::sleep(deadline - Instant::now());
        std::process::exit(0);
    });
    let mut carry = Vec::new();
    while let Ok(Some(ev)) = stream.next_event() {
        let t = start.elapsed().as_secs_f64();
        match ev {
            AttachEvent::Output { seq, data } => {
                if only_osc {
                    carry.extend(&data);
                    for osc in take_oscs(&mut carry) {
                        println!("{t:7.3} OSC {}", escape(&osc));
                    }
                } else {
                    println!("{t:7.3} @{seq} {}", escape(&data));
                }
            }
            other => println!("{t:7.3} {other:?}"),
        }
    }
}
