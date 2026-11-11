//! Card D-T2 (docs/20-refactor/daemon.md §3): the blocking client lives in
//! `pod-client` only, and podd's own copies of `client.rs`, `transport.rs`,
//! `endpoint.rs` and `detach.rs` are gone. The old paths (`podd::client`,
//! `podd::transport`, `podd::endpoint`, `podd::detach`, `podd::now_ms`)
//! must name the very same items, not copies: a `Transport` made through one
//! path drives a `Client` written against the other with no conversion.
//!
//! The compiler does most of the checking; the runtime tests compare the
//! constants and pure functions and move one live connection across paths.

use std::io::Write;
use std::process::Command;

use serde_json::{Value, json};

/// Implemented only when `Self` and `T` are one type.
trait Same<T: ?Sized> {}
impl<T: ?Sized> Same<T> for T {}

/// Compiles only when `A` and `B` are one type.
fn same<A: ?Sized + Same<B>, B: ?Sized>() {}

#[test]
fn client_items_are_the_pod_client_items() {
    use pod_client::client as new;
    use podd::client as old;
    same::<old::Client, new::Client>();
    same::<old::ClientError, new::ClientError>();
    same::<old::StartLock, new::StartLock>();
    same::<old::AttachEvent, new::AttachEvent>();
    same::<old::AttachStream, new::AttachStream>();
    same::<old::StatusStreamEvent, new::StatusStreamEvent>();
    same::<old::StatusStream, new::StatusStream>();
    same::<old::SubscribeEvent, new::SubscribeEvent>();
    #[cfg(windows)]
    same::<old::ProcessWatch, new::ProcessWatch>();
    assert_eq!(old::START_TIMEOUT, new::START_TIMEOUT);
    assert_eq!(old::ENV_DAEMON_EXE, new::ENV_DAEMON_EXE);
    // A function of either path, typed with the other path's items.
    let _: fn(&str) -> Result<old::Client, new::ClientError> = new::Client::connect;
}

#[test]
fn transport_items_are_the_pod_client_items() {
    use pod_client::transport as new;
    use podd::transport as old;
    same::<old::Transport, new::Transport>();
    same::<old::LineReader, new::LineReader>();
    same::<old::LineWriter, new::LineWriter>();
    same::<old::FrameError, new::FrameError>();
    assert_eq!(old::LOCAL_MAX_LINE, new::LOCAL_MAX_LINE);
    assert_eq!(old::REMOTE_MAX_LINE, new::REMOTE_MAX_LINE);
    // security-01: the identification-only SQOS flags of every pipe client.
    #[cfg(windows)]
    assert_eq!(old::SQOS_IDENTIFICATION, new::SQOS_IDENTIFICATION);
    let _: fn(&str) -> std::io::Result<old::Transport> = new::Transport::open_pipe;
    let _: fn(&str) -> std::io::Result<std::fs::File> = old::open_pipe;
}

#[cfg(windows)]
#[test]
fn detach_items_are_the_pod_client_items() {
    use pod_client::detach as new;
    use podd::detach as old;
    same::<old::DetachedChild, new::DetachedChild>();
    let _: fn(&Command) -> std::io::Result<new::DetachedChild> = old::spawn_detached;
    let _: fn(&mut Command) -> std::io::Result<old::DetachedChild> =
        pod_client::client::spawn_detached;
    let program = std::ffi::OsStr::new(r"C:\Program Files\pod\podd.exe");
    let args = [std::ffi::OsStr::new("--pipe"), std::ffi::OsStr::new("a b")];
    assert_eq!(
        old::command_line(program, &args),
        new::command_line(program, &args)
    );
}

#[test]
fn endpoint_items_are_the_pod_client_items() {
    use pod_client::endpoint as new;
    use podd::endpoint as old;
    assert_eq!(old::PIPE_PREFIX, new::PIPE_PREFIX);
    assert_eq!(old::ENV_SOCKET, new::ENV_SOCKET);
    assert_eq!(old::ENV_TERMINAL_ID, new::ENV_TERMINAL_ID);
    for name in ["podd-test", r"\\.\pipe\podd-a b", "x/y:z"] {
        assert_eq!(old::pipe_path(name), new::pipe_path(name));
        assert_eq!(old::pipe_key(name), new::pipe_key(name));
        assert_eq!(old::holder_pipe(name, "t1"), new::holder_pipe(name, "t1"));
        assert_eq!(old::sanitize(name), new::sanitize(name));
    }
    assert_eq!(old::pipe_key(r"\\.\pipe\podd-a b"), "podd-a_b");
    let before = pod_client::now_ms();
    let after = podd::now_ms();
    assert!(before <= after, "{before} > {after}");
}

/// One connection crosses both paths: a transport built through
/// `podd::transport` drives a `pod_client` client, comes back out of it and
/// drives a `podd` client.
#[test]
fn one_connection_moves_between_old_and_new_paths() {
    let (c_read, s_write) = std::io::pipe().unwrap();
    let (s_read, c_write) = std::io::pipe().unwrap();
    let server = std::thread::spawn(move || {
        let mut r = podd::transport::LineReader::new(s_read, 1024);
        let mut w = s_write;
        let mut seen = Vec::new();
        while let Ok(Some(line)) = r.read_line() {
            let req: Value = serde_json::from_str(&line).unwrap();
            let reply =
                json!({"jsonrpc": "2.0", "id": req["id"], "result": {"echo": req["method"]}});
            writeln!(w, "{reply}").unwrap();
            seen.push((req["id"].clone(), req["method"].clone()));
        }
        seen
    });

    let t = podd::transport::Transport::new(c_read, c_write, 1024);
    let mut new = pod_client::client::Client::from_transport("fake", t);
    let got = new.call_raw("one", Value::Null).unwrap();
    assert_eq!(got, json!({"echo": "one"}));

    let t: podd::transport::Transport = new.into_transport();
    let mut old = podd::client::Client::from_transport("fake", t);
    assert_eq!(old.pipe(), "fake");
    let got = old.call_raw("two", Value::Null).unwrap();
    assert_eq!(got, json!({"echo": "two"}));

    drop(old);
    let seen = server.join().unwrap();
    // A fresh client numbers from 1 again: it is a new RPC layer on the same
    // line transport, as `Client::from_transport` documents.
    assert_eq!(
        seen,
        vec![(json!(1), json!("one")), (json!(1), json!("two"))]
    );
}
