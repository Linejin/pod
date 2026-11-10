//! Daemon start with holder records it cannot re-attach (daemon-03/04).
//!
//! The holders here are fakes served by this test: one answers with a broken
//! frame, one keeps its only pipe instance busy forever. Neither may hold up
//! the start, and no new terminal may reuse an id a record still names.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use podd::protocol::*;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

fn write_record(dir: &Path, id: &str, pipe: &str) {
    let rec = serde_json::json!({
        "id": id, "name": null, "pipe": pipe,
        "holder_pid": 4_000_000_000u32, "child_pid": 4_000_000_001u32, "created_ms": 1,
    });
    std::fs::write(dir.join(format!("{id}.json")), rec.to_string()).unwrap();
}

#[test]
fn unreachable_holders_keep_their_ids_and_do_not_block_the_start() {
    let name = unique("rediscover");
    let pipe = format!(r"\\.\pipe\{name}");
    let state_dir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&state_dir).unwrap();
    let broken = format!(r"\\.\pipe\{name}-broken");
    let busy = format!(r"\\.\pipe\{name}-busy");

    let rt = tokio::runtime::Runtime::new().unwrap();
    // A "holder" that answers every connection with an invalid frame.
    let broken_server = {
        let broken = broken.clone();
        let mut server = rt.block_on(async {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create(&broken)
                .unwrap()
        });
        rt.spawn(async move {
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                let next = ServerOptions::new().create(&broken).unwrap();
                let mut conn = std::mem::replace(&mut server, next);
                tokio::spawn(async move {
                    let _ = conn.write_all(&[0, 0, 0, 0]).await; // length 0: invalid
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        })
    };
    // A "holder" whose only pipe instance stays taken: ERROR_PIPE_BUSY forever.
    let (_busy_server, _busy_occupant) = rt.block_on(async {
        let s = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .create(&busy)
            .unwrap();
        let c = ClientOptions::new().open(&busy).unwrap();
        (s, c)
    });

    write_record(&state_dir, "t1", &broken);
    write_record(&state_dir, "t7", &busy);
    // An unreadable record still reserves the id in its file name.
    std::fs::write(state_dir.join("t4.json"), b"{ not json").unwrap();

    let t0 = Instant::now();
    let daemon = Daemon::start_on(pipe, state_dir.clone());
    let mut c = daemon.client();
    // The pipe exists before rediscovery; the first answer comes after it.
    c.call::<ServerStatus>(&Empty {}).unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(9),
        "the first request was answered after {:?}",
        t0.elapsed()
    );
    let t = powershell(&mut c, "fresh");
    assert_eq!(t.id, "t8", "a new terminal must not reuse a reserved id");
    // The unreachable holders' records are kept (they may still be alive).
    for id in ["t1", "t4", "t7"] {
        assert!(
            state_dir.join(format!("{id}.json")).exists(),
            "{id}.json removed"
        );
    }
    // They are listed as unreachable (with a reason) next to the new one.
    let list = c.call::<TerminalList>(&Empty {}).unwrap().terminals;
    let states: Vec<(&str, TerminalState)> =
        list.iter().map(|t| (t.id.as_str(), t.state)).collect();
    use TerminalState::*;
    assert_eq!(
        states,
        [
            ("t1", Unreachable),
            ("t4", Unreachable),
            ("t7", Unreachable),
            ("t8", Running)
        ],
        "{list:?}"
    );
    assert!(
        list[..3].iter().all(|t| t.unreachable.is_some()),
        "{list:?}"
    );
    broken_server.abort();
}
