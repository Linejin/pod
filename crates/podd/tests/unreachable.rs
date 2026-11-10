//! Holders a restarted daemon cannot reach (daemon-03 follow-up): listed as
//! `unreachable` with a reason, retried in the background, re-attached when
//! they answer, and forgotten only by `terminal.kill`.
//!
//! The holders are fakes served by this test: a pipe whose only instance is
//! taken (`ERROR_PIPE_BUSY`), freed later to answer like a real holder.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use podd::client::Client;
use podd::holder_proto::{ExitInfo, Frame, HolderHello, read_frame};
use podd::protocol::*;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

fn write_record(dir: &Path, id: &str, name: &str, pipe: &str) {
    let rec = serde_json::json!({
        "id": id, "name": name, "pipe": pipe,
        "holder_pid": 4_000_000_000u32, "child_pid": 4_000_000_001u32, "created_ms": 1,
    });
    std::fs::write(dir.join(format!("{id}.json")), rec.to_string()).unwrap();
}

/// A pipe with one instance, taken by a client: every open is busy.
fn busy_pipe(rt: &tokio::runtime::Runtime, pipe: &str) -> (NamedPipeServer, NamedPipeClient) {
    rt.block_on(async {
        let s = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .create(pipe)
            .unwrap();
        let c = ClientOptions::new().open(pipe).unwrap();
        (s, c)
    })
}

/// Serve one daemon connection like a holder of terminal `id`: hello,
/// empty snapshot, then answer kill with an exit. Returns the frames the
/// daemon sent, once it disconnected.
async fn serve_as_holder(server: NamedPipeServer, id: &str) -> Vec<Frame> {
    server.connect().await.unwrap();
    let hello = HolderHello {
        version: 3,
        id: id.into(),
        name: Some("late".into()),
        argv: vec!["fake".into()],
        cwd: "C:\\".into(),
        created_ms: 1,
        holder_pid: std::process::id(),
        child_pid: std::process::id(),
        cols: 80,
        rows: 24,
        conpty: "inbox".into(),
        parent_terminal: None,
        parent_task: None,
    };
    let (mut rd, mut wr) = tokio::io::split(server);
    wr.write_all(&Frame::Hello(hello).encode()).await.unwrap();
    wr.write_all(
        &Frame::Snapshot {
            seq: 0,
            data: Vec::new(),
        }
        .encode(),
    )
    .await
    .unwrap();
    let mut seen = Vec::new();
    while let Ok(Some(f)) = read_frame(&mut rd).await {
        seen.push(f.clone());
        if f == Frame::Kill {
            let exit = ExitInfo {
                seq: 0,
                exit_code: Some(1),
                killed: true,
                exited_ms: 2,
            };
            let _ = wr.write_all(&Frame::Exited(exit).encode()).await;
        }
    }
    seen
}

/// A v18 client's view of a terminal: its `state` has no `unreachable`, so
/// decoding one fails the way an old app or CLI would (ui-editor-03).
#[derive(serde::Deserialize, Debug)]
struct OldInfo {
    id: String,
    state: OldState,
    #[serde(default)]
    lost: bool,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum OldState {
    Running,
    Exited,
}

#[derive(serde::Deserialize, Debug)]
struct OldList {
    terminals: Vec<OldInfo>,
}

fn list(c: &mut Client) -> Vec<TerminalInfo> {
    c.call::<TerminalList>(&Empty {}).unwrap().terminals
}

fn find(c: &mut Client, id: &str) -> Option<TerminalInfo> {
    list(c).into_iter().find(|t| t.id == id)
}

/// Wait until `f` holds for terminal `id` (or panic after `secs`).
fn wait_for(
    c: &mut Client,
    id: &str,
    secs: u64,
    f: impl Fn(&TerminalInfo) -> bool,
) -> TerminalInfo {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let t = find(c, id);
        if let Some(t) = t.as_ref().filter(|t| f(t)) {
            return t.clone();
        }
        assert!(Instant::now() < deadline, "{id} never matched: {t:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn hello(c: &mut Client, max: u32) {
    let h = c
        .call::<ConnectionHello>(&HelloParams {
            min_protocol: 12,
            max_protocol: max,
            require: Vec::new(),
            client: Some("unreachable test".into()),
        })
        .unwrap();
    assert_eq!(h.protocol, max);
}

#[test]
fn busy_holder_is_listed_then_reattached_when_it_answers() {
    let name = unique("unreach-late");
    let state_dir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&state_dir).unwrap();
    let holder = format!(r"\\.\pipe\{name}-t2");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (server, occupant) = busy_pipe(&rt, &holder);
    write_record(&state_dir, "t2", "late", &holder);

    let daemon = Daemon::start_full(
        format!(r"\\.\pipe\{name}"),
        state_dir.clone(),
        &[],
        &[("POD_HOLDER_RETRY_MS", "200"), ("POD_HOLDER_RETRIES", "20")],
    );
    let mut c = daemon.client();
    let t = find(&mut c, "t2").expect("unreachable holder listed");
    assert_eq!(t.state, TerminalState::Unreachable, "{t:?}");
    assert_eq!(t.name.as_deref(), Some("late"));
    assert!(
        t.unreachable
            .as_deref()
            .is_some_and(|r| r.contains("timed out")),
        "{t:?}"
    );
    // Nothing can be sent to it; the error says why.
    let err = c
        .call::<TerminalRead>(&ReadParams {
            target: TerminalRef::parse("t2"),
            mode: ReadMode::default(),
            lines: None,
            raw: false,
        })
        .unwrap_err();
    assert!(err.to_string().contains("unreachable"), "{err}");

    // The holder frees its pipe and answers: the retry re-attaches it.
    drop(occupant);
    rt.block_on(async { server.disconnect().unwrap() });
    rt.spawn(serve_as_holder(server, "t2"));
    let t = wait_for(&mut c, "t2", 30, |t| t.state == TerminalState::Running);
    assert_eq!((t.unreachable, t.argv), (None, vec!["fake".to_string()]));
    assert!(state_dir.join("t2.json").exists(), "record kept");
    drop(c);
    drop(daemon);
}

#[test]
fn never_reachable_holder_stays_listed_until_kill_forgets_it() {
    let name = unique("unreach-never");
    let state_dir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&state_dir).unwrap();
    let holder = format!(r"\\.\pipe\{name}-t5");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_server, _occupant) = busy_pipe(&rt, &holder);
    write_record(&state_dir, "t5", "stuck", &holder);
    std::fs::write(state_dir.join("t5.token"), "00").unwrap();

    let daemon = Daemon::start_full(
        format!(r"\\.\pipe\{name}"),
        state_dir.clone(),
        &[],
        &[("POD_HOLDER_RETRY_MS", "100"), ("POD_HOLDER_RETRIES", "2")],
    );
    let mut c = daemon.client();
    // Start attempt + 2 retries, each up to 5 s on the busy pipe.
    let t = wait_for(&mut c, "t5", 40, |t| {
        t.unreachable
            .as_deref()
            .is_some_and(|r| r.contains("gave up after 3 attempts"))
    });
    assert_eq!(t.state, TerminalState::Unreachable);

    // A client that said hello with protocol 18 never sees the new state,
    // decoded with v18 types (no `unreachable` variant).
    let mut old = daemon.client();
    hello(&mut old, 18);
    let raw = old
        .call_raw(TerminalList::NAME, serde_json::json!({}))
        .unwrap();
    let l: OldList = serde_json::from_value(raw).expect("v18 list decodes");
    let t = l.terminals.iter().find(|t| t.id == "t5").unwrap();
    assert_eq!((&t.state, t.lost), (&OldState::Exited, true), "{t:?}");
    let mut new = daemon.client();
    hello(&mut new, PROTOCOL_VERSION);
    assert_eq!(
        find(&mut new, "t5").unwrap().state,
        TerminalState::Unreachable
    );

    // Its id stays reserved while it is listed.
    let fresh = powershell(&mut c, "fresh");
    assert_eq!(fresh.id, "t6");

    // `pod kill` from the v18 client gives up: its answer decodes with v18
    // types (ui-editor-03); unlisted, record and token removed.
    let raw = old
        .call_raw(TerminalKill::NAME, serde_json::json!({"name": "stuck"}))
        .unwrap();
    let k: OldInfo = serde_json::from_value(raw).expect("v18 kill answer decodes");
    assert_eq!(
        (k.id.as_str(), &k.state, k.lost),
        ("t5", &OldState::Exited, true)
    );
    assert!(find(&mut c, "t5").is_none());
    assert!(!state_dir.join("t5.json").exists(), "record removed");
    assert!(!state_dir.join("t5.token").exists(), "token removed");
    assert_eq!(
        rpc_code(c.call::<TerminalKill>(&TerminalRef::parse("t5"))),
        codes::TERMINAL_NOT_FOUND
    );
    drop(c);
    drop(daemon);
}

/// daemon-security-04: `server.stop kill:true` while a retry round is about
/// to re-attach a holder. The round counts as a start, so the stop waits for
/// it and the re-attached holder gets Kill and Release like any other. A
/// debug-only knob holds the round open so the stop lands inside it.
#[test]
fn stop_kill_waits_for_a_retry_round_and_ends_its_holder() {
    let name = unique("unreach-stop");
    let state_dir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&state_dir).unwrap();
    let holder = format!(r"\\.\pipe\{name}-t2");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (server, occupant) = busy_pipe(&rt, &holder);
    write_record(&state_dir, "t2", "late", &holder);

    let mut daemon = Daemon::start_full(
        format!(r"\\.\pipe\{name}"),
        state_dir.clone(),
        &[],
        &[
            ("POD_HOLDER_RETRY_MS", "10"),
            ("POD_HOLDER_RETRIES", "3"),
            ("POD_HOLDER_TEST_RETRY_ROUND_DELAY_MS", "3000"),
        ],
    );
    let mut c = daemon.client();
    let t = find(&mut c, "t2").expect("listed");
    assert_eq!(t.state, TerminalState::Unreachable, "{t:?}");

    // The holder answers from now on; the first round is held open.
    drop(occupant);
    rt.block_on(async { server.disconnect().unwrap() });
    let served = rt.spawn(serve_as_holder(server, "t2"));
    std::thread::sleep(Duration::from_millis(500));
    let _ = c.call::<ServerStop>(&StopParams { kill: true });
    daemon.wait_exit();

    let frames = rt
        .block_on(async { tokio::time::timeout(Duration::from_secs(10), served).await })
        .expect("the daemon disconnected from the holder")
        .unwrap();
    assert!(frames.contains(&Frame::Kill), "never killed: {frames:?}");
    assert!(
        frames.contains(&Frame::Release),
        "never released: {frames:?}"
    );
}
