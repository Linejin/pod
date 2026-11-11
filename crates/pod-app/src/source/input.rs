//! The input connection: typed keys and terminal-view query replies, in
//! order, on a connection of their own ([`input_loop`]).

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use podd::client::Client;
use podd::protocol as p;

use super::control::is_transport;

/// Bytes for a terminal's input, on the input connection ([`input_loop`]).
pub(super) enum InputCmd {
    /// What the user typed or pasted.
    Keys(String, Vec<u8>),
    /// The view's answer to a query in the output (OSC 10/11/4, CSI 14 t),
    /// stamped when the view produced it.
    Reply(String, Vec<u8>, Instant),
}

/// How long a query reply may wait before it is dropped. A program reads the
/// reply only while it waits for it; after that (Codex on Windows reads
/// console key events) the bytes arrive as typed text (E2E B4).
pub const REPLY_MAX_AGE: Duration = Duration::from_millis(500);

/// Decides which view replies go to the program (E2E B4).
///
/// - **Stale**: older than [`REPLY_MAX_AGE`] when the input thread gets to it
///   (it was stuck behind a reconnect): dropped instead of typed into the agent.
/// - **Duplicate**: every panel showing the terminal answers the same query
///   with the same bytes; the program waits for one answer, so the same reply
///   for the same terminal within `REPLY_MAX_AGE` of the last one is dropped.
///   (podd v7 drops duplicates of the holder's DA/DSR/OSC 7501 answers, not of
///   color replies, which the holder does not produce.)
#[derive(Default)]
pub struct ReplyFilter {
    last: HashMap<String, (Vec<u8>, Instant)>,
}

impl ReplyFilter {
    pub fn admit(&mut self, id: &str, bytes: &[u8], born: Instant, now: Instant) -> bool {
        if now.saturating_duration_since(born) > REPLY_MAX_AGE {
            return false;
        }
        if let Some((prev, at)) = self.last.get(id)
            && prev == bytes
            && born.saturating_duration_since(*at) <= REPLY_MAX_AGE
        {
            return false;
        }
        self.last
            .retain(|_, (_, at)| now.saturating_duration_since(*at) <= REPLY_MAX_AGE);
        self.last.insert(id.to_string(), (bytes.to_vec(), born));
        true
    }
}

/// The input connection: `terminal.input` for typed keys and view replies,
/// in order, on a connection of its own. On the control connection a reply
/// waited behind `task.list` (git status of every worktree) right after an
/// app-created task started, reached Codex after its color query timed out
/// and showed up as `]10;rgb:…` in its composer (E2E B4). Connects lazily,
/// never starts podd; input is dropped while podd is unreachable.
pub(super) fn input_loop(pipe: String, rx: std_mpsc::Receiver<InputCmd>) {
    let mut client: Option<Client> = None;
    let mut filter = ReplyFilter::default();
    let debug = std::env::var_os("POD_APP_DEBUG").is_some();
    while let Ok(first) = rx.recv() {
        let (id, bytes) = match first {
            InputCmd::Keys(id, mut bytes) => {
                // Typing bursts and pastes: merge what is already queued for the same terminal.
                let mut next = None;
                while let Ok(n) = rx.try_recv() {
                    match n {
                        InputCmd::Keys(i, b) if i == id => bytes.extend_from_slice(&b),
                        other => {
                            next = Some(other);
                            break;
                        }
                    }
                }
                send_input(&pipe, &mut client, &id, &bytes);
                match next {
                    None => continue,
                    Some(InputCmd::Keys(i, b)) => (i, b),
                    Some(InputCmd::Reply(i, b, born)) => {
                        match admit_reply(&mut filter, &i, b, born, debug) {
                            Some(b) => (i, b),
                            None => continue,
                        }
                    }
                }
            }
            InputCmd::Reply(i, b, born) => match admit_reply(&mut filter, &i, b, born, debug) {
                Some(b) => (i, b),
                None => continue,
            },
        };
        send_input(&pipe, &mut client, &id, &bytes);
    }
}

fn admit_reply(
    filter: &mut ReplyFilter,
    id: &str,
    bytes: Vec<u8>,
    born: Instant,
    debug: bool,
) -> Option<Vec<u8>> {
    let now = Instant::now();
    let ok = filter.admit(id, &bytes, born, now);
    if debug {
        let verdict = if ok { "sent" } else { "dropped" };
        println!(
            "[pod-app] reply {verdict} for {id} after {} ms: {}",
            now.saturating_duration_since(born).as_millis(),
            bytes.escape_ascii()
        );
    }
    ok.then_some(bytes)
}

pub(super) fn send_input(pipe: &str, client: &mut Option<Client>, id: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if client.is_none() {
        *client = Client::connect(pipe).ok();
    }
    let Some(c) = client.as_mut() else { return };
    let params = p::InputParams {
        target: p::TerminalRef::id(id),
        data: p::b64_encode(bytes),
    };
    if let Err(e) = c.call::<p::TerminalInput>(&params)
        && is_transport(&e)
    {
        *client = None;
    }
}
