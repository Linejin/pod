//! One client connection: line framing, hook lines, the stop check, the
//! protocol chosen by `connection.hello` (every later message is downgraded
//! to it), and the switch to an output stream (`terminal.attach`; the
//! `status.subscribe` stream lives in `status.rs`).

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::NamedPipeServer;

use super::rpc::{self, Mode};
use super::runtime::stopping_err;
use super::terminal::{self, StreamMsg};
use super::{Daemon, log, params, status};
use crate::protocol::*;

/// Requests larger than this are rejected (protects the line reader).
pub(in crate::server) const MAX_LINE: usize = 4 * 1024 * 1024;
// pod-hook caps its lines to this (daemon-05); a larger hook line is dropped.
const _: () = assert!(MAX_LINE >= pod_status::hook::MAX_HOOK_LINE);
const DEFAULT_ATTACH_SCROLLBACK: usize = 1000;

tokio::task_local! {
    /// Protocol chosen by `connection.hello` on the connection this task
    /// serves; none = this build's (local clients ship with the daemon and
    /// send no hello). [`write_json`] downgrades every message it writes
    /// there, so responses and stream notifications alike match it.
    static CONN_PROTOCOL: std::cell::Cell<Option<u32>>;
}

pub(in crate::server) async fn serve_connection(
    daemon: Arc<Daemon>,
    conn: NamedPipeServer,
) -> anyhow::Result<()> {
    CONN_PROTOCOL
        .scope(std::cell::Cell::new(None), handle_connection(daemon, conn))
        .await
}

async fn handle_connection(daemon: Arc<Daemon>, conn: NamedPipeServer) -> anyhow::Result<()> {
    let (read, mut write) = tokio::io::split(conn);
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        let n = read_line_limited(&mut reader, &mut line, MAX_LINE).await?;
        if n == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        if status::try_ingest_hook(&daemon, &line) {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = err_response(
                    Value::Null,
                    RpcError::new(codes::PARSE_ERROR, format!("parse error: {e}")),
                );
                write_json(&mut write, &resp).await?;
                continue;
            }
        };
        // Counted before the stop check: either the stop sees this request
        // and waits for it, or this request sees the stop and is refused.
        let guard = daemon.begin_request();
        if daemon.is_stopping() {
            if let Some(id) = req.id {
                write_json(&mut write, &err_response(id, stopping_err())).await?;
            }
            continue;
        }
        // Streams switch before the JSON-RPC version check, and only for a
        // request with an id (a notification goes to the dispatch).
        let mode = rpc::mode(&req.method);
        if mode == Mode::Attach
            && let Some(id) = req.id.clone()
        {
            drop(guard);
            // The connection becomes an output stream until either side closes it.
            return daemon.stream_attach(id, req.params, reader, write).await;
        }
        if mode == Mode::Subscribe
            && let Some(id) = req.id.clone()
        {
            drop(guard);
            return status::stream_subscribe(daemon, id, req.params, reader, write).await;
        }
        let method = req.method.clone();
        let Some(resp) = daemon.clone().handle_request(req).await else {
            continue;
        };
        if method == ConnectionHello::NAME
            && let Some(v) = resp
                .result
                .as_ref()
                .and_then(|r| r.get("protocol"))
                .and_then(Value::as_u64)
        {
            let _ = CONN_PROTOCOL.try_with(|c| c.set(Some(v as u32)));
        }
        write_json(&mut write, &resp).await?;
        drop(guard);
    }
}

/// `read_line` that refuses a line longer than `max` bytes (without the
/// `\n`) **before** buffering it, so a peer that never sends a newline cannot
/// make the daemon allocate more than `max`. `buf` is cleared first and gets
/// the line with its `\n`; returns the bytes read (0 at EOF). A line cut by
/// EOF is returned as is (the JSON parse then fails), like `read_line`.
pub(crate) async fn read_line_limited<R: AsyncBufRead + Unpin>(
    r: &mut R,
    buf: &mut String,
    max: usize,
) -> anyhow::Result<usize> {
    buf.clear();
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            break;
        }
        let (take, done) = match avail.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (avail.len(), false),
        };
        if bytes.len() + take - usize::from(done) > max {
            anyhow::bail!("request line longer than {max} bytes");
        }
        bytes.extend_from_slice(&avail[..take]);
        r.consume(take);
        if done {
            break;
        }
    }
    let n = bytes.len();
    *buf = String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "request line is not UTF-8")
    })?;
    Ok(n)
}

/// Read and drop whatever the client sends on a streaming connection
/// (attach, subscribe) without buffering it. Returns the bytes dropped, 0 at
/// EOF. Cancel-safe (used in `select!`).
pub(crate) async fn discard_input<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<usize> {
    let n = r.fill_buf().await?.len();
    r.consume(n);
    Ok(n)
}

pub(in crate::server) async fn write_json<W: AsyncWriteExt + Unpin, T: Serialize>(
    w: &mut W,
    v: &T,
) -> anyhow::Result<()> {
    // ui-editor-03: one place for every message (any response, attach and
    // subscribe notifications), not a per-method list.
    let protocol = CONN_PROTOCOL.try_with(|c| c.get()).ok().flatten();
    let mut out = match protocol {
        Some(p) if p < UNREACHABLE_PROTOCOL => {
            let mut value = serde_json::to_value(v)?;
            downgrade_json(&mut value, p);
            serde_json::to_string(&value)?
        }
        _ => serde_json::to_string(v)?,
    };
    out.push('\n');
    w.write_all(out.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

pub(in crate::server) fn ok_response(id: Value, result: Value) -> Response {
    Response {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result: Some(result),
        error: None,
    }
}

pub(in crate::server) fn err_response(id: Value, error: RpcError) -> Response {
    Response {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result: None,
        error: Some(error),
    }
}

/// Does this connection use the v20 remote framing (its hello chose 20 or
/// newer)? Local clients send no hello.
pub(in crate::server) fn remote_framing() -> bool {
    CONN_PROTOCOL
        .try_with(|c| c.get())
        .ok()
        .flatten()
        .is_some_and(|p| p >= CHUNKED_ATTACH_PROTOCOL)
}

/// The per-attach queue limit: the daemon's, or for the v20 remote framing
/// the 2 MiB remote one when that is smaller (docs/17 §2.3).
pub(in crate::server) fn attach_queue_for(remote: bool, daemon_limit: usize) -> usize {
    if remote {
        daemon_limit.min(REMOTE_ATTACH_QUEUE)
    } else {
        daemon_limit
    }
}

/// The notifications for one stream message. Output is split into pieces of
/// at most `chunk` bytes with contiguous `seq`s (the clients' contract since
/// v2). The flag says the message is the last of the stream.
fn stream_notifications(id: &str, msg: StreamMsg, chunk: usize) -> (Vec<Notification>, bool) {
    match msg {
        StreamMsg::Output { seq, data } => {
            let mut at = seq;
            let out = data
                .chunks(chunk.max(1))
                .map(|piece| {
                    let n = notification(
                        NOTIFY_OUTPUT,
                        OutputEvent {
                            id: id.to_string(),
                            seq: at,
                            data: b64_encode(piece),
                        },
                    );
                    at += piece.len() as u64;
                    n
                })
                .collect();
            (out, false)
        }
        StreamMsg::Resized { seq, cols, rows } => (
            vec![notification(
                NOTIFY_RESIZED,
                ResizedEvent {
                    id: id.to_string(),
                    seq,
                    cols,
                    rows,
                },
            )],
            false,
        ),
        StreamMsg::Exited(e) => (
            vec![notification(
                NOTIFY_EXITED,
                ExitedEvent {
                    id: id.to_string(),
                    seq: e.seq,
                    exit_code: e.exit_code,
                    killed: e.killed,
                },
            )],
            true,
        ),
    }
}

pub(in crate::server) fn notification<T: Serialize>(method: &str, params: T) -> Notification {
    Notification {
        jsonrpc: JSONRPC_VERSION.into(),
        method: method.into(),
        params: serde_json::to_value(params).unwrap_or(Value::Null),
    }
}

impl Daemon {
    /// `terminal.attach`: reply with a snapshot, then stream notifications
    /// until the client disconnects or the terminal exits.
    async fn stream_attach(
        &self,
        id: Value,
        p: Value,
        mut reader: BufReader<tokio::io::ReadHalf<NamedPipeServer>>,
        mut write: tokio::io::WriteHalf<NamedPipeServer>,
    ) -> anyhow::Result<()> {
        let prepared = async {
            let p: AttachParams = params(p)?;
            let t = self.resolve(&p.target)?;
            if let (Some(c), Some(r)) = (p.cols, p.rows)
                && t.is_running()
            {
                self.resize_to(&t, c, r).await?;
            }
            Ok::<_, RpcError>((t, p.scrollback.unwrap_or(DEFAULT_ATTACH_SCROLLBACK)))
        }
        .await;
        let (t, scrollback) = match prepared {
            Ok(x) => x,
            Err(e) => return write_json(&mut write, &err_response(id, e)).await,
        };
        // v20 remote framing (docs/17 R4) for a connection whose hello chose
        // 20 or newer: chunked snapshot, output chunks of at most 32 KiB and
        // the 2 MiB queue. Local attaches and older hellos are unchanged.
        let remote = remote_framing();
        let a = t.attach_with(terminal::AttachSpec {
            scrollback,
            queue_limit: attach_queue_for(remote, terminal::attach_queue_limit()),
            max_snapshot: remote.then_some(MAX_SNAPSHOT_BYTES),
        });
        let (seq, mut sub) = (a.seq, a.rx);
        if remote {
            let chunks: Vec<&[u8]> = a.snapshot.chunks(REMOTE_OUTPUT_CHUNK).collect();
            let total = chunks.len() as u32;
            let result = AttachResult {
                terminal: t.info(),
                seq,
                snapshot: String::new(),
                snapshot_chunks: Some(total),
                snapshot_bytes: Some(a.snapshot.len() as u64),
                scrollback_omitted: (a.scrollback_omitted > 0).then_some(a.scrollback_omitted),
            };
            write_json(&mut write, &ok_response(id, serde_json::to_value(result)?)).await?;
            for (index, data) in chunks.into_iter().enumerate() {
                let n = notification(
                    NOTIFY_SNAPSHOT,
                    SnapshotChunkEvent {
                        id: t.id.clone(),
                        seq,
                        index: index as u32,
                        total,
                        data: b64_encode(data),
                    },
                );
                write_json(&mut write, &n).await?;
            }
            log(&format!(
                "attach {} at seq {seq} (remote: {} bytes in {total} chunks{})",
                t.id,
                a.snapshot.len(),
                if a.scrollback_omitted > 0 {
                    format!(", {} scrollback lines left out", a.scrollback_omitted)
                } else {
                    String::new()
                }
            ));
        } else {
            let result = AttachResult {
                terminal: t.info(),
                seq,
                snapshot: b64_encode(&a.snapshot),
                snapshot_chunks: None,
                snapshot_bytes: None,
                scrollback_omitted: None,
            };
            write_json(&mut write, &ok_response(id, serde_json::to_value(result)?)).await?;
            log(&format!("attach {} at seq {seq}", t.id));
        }
        drop(a.snapshot);
        let chunk = if remote {
            REMOTE_OUTPUT_CHUNK
        } else {
            usize::MAX
        };

        loop {
            tokio::select! {
                msg = sub.rx.recv() => {
                    // Dropped for falling behind: say so instead of writing the
                    // rest of the queue, then close (the client re-attaches).
                    if let Some(lag) = sub.lag() {
                        let n = notification(NOTIFY_LAGGED, LaggedEvent {
                            id: t.id.clone(),
                            seq: lag.seq,
                            queued_bytes: lag.queued_bytes,
                            limit_bytes: sub.shared.limit as u64,
                        });
                        log(&format!(
                            "attach {} lagged at seq {} ({} bytes queued); dropped",
                            t.id, lag.seq, lag.queued_bytes
                        ));
                        let _ = write_json(&mut write, &n).await;
                        break;
                    }
                    let Some(msg) = msg else { break };
                    sub.taken(&msg);
                    let (out, last) = stream_notifications(&t.id, msg, chunk);
                    let mut failed = false;
                    for n in &out {
                        if write_json(&mut write, n).await.is_err() {
                            failed = true;
                            break;
                        }
                    }
                    if failed || last {
                        break;
                    }
                }
                // Anything the client sends is ignored; EOF means detach.
                r = discard_input(&mut reader) => {
                    if !matches!(r, Ok(n) if n > 0) {
                        break;
                    }
                }
            }
        }
        log(&format!("detach {}", t.id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(n: &Notification) -> OutputEvent {
        assert_eq!(n.method, NOTIFY_OUTPUT);
        serde_json::from_value(n.params.clone()).unwrap()
    }

    #[test]
    fn remote_attaches_get_the_smaller_queue() {
        const MIB: usize = 1024 * 1024;
        assert_eq!(attach_queue_for(true, 16 * MIB), 2 * MIB);
        assert_eq!(attach_queue_for(true, 32 * 1024), 32 * 1024);
        assert_eq!(attach_queue_for(false, 16 * MIB), 16 * MIB);
    }

    /// v20 remote framing: output pieces of at most 32 KiB with contiguous
    /// offsets; without a chunk limit one notification as before.
    #[test]
    fn output_is_split_into_contiguous_chunks() {
        let data: Arc<[u8]> = (0..70_000u32).map(|i| i as u8).collect::<Vec<_>>().into();
        let msg = StreamMsg::Output {
            seq: 10,
            data: data.clone(),
        };
        let (out, last) = stream_notifications("t1", msg.clone(), REMOTE_OUTPUT_CHUNK);
        assert!(!last);
        let events: Vec<OutputEvent> = out.iter().map(output).collect();
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [10, 10 + 32_768, 10 + 65_536]);
        let mut joined = Vec::new();
        for e in &events {
            let piece = b64_decode(&e.data).unwrap();
            assert!(piece.len() <= REMOTE_OUTPUT_CHUNK);
            joined.extend(piece);
        }
        assert_eq!(joined, &data[..]);

        let (out, _) = stream_notifications("t1", msg, usize::MAX);
        assert_eq!(out.len(), 1);
        assert_eq!(b64_decode(&output(&out[0]).data).unwrap(), &data[..]);

        let (out, last) = stream_notifications(
            "t1",
            StreamMsg::Resized {
                seq: 3,
                cols: 80,
                rows: 24,
            },
            REMOTE_OUTPUT_CHUNK,
        );
        assert_eq!((out.len(), last), (1, false));
    }
}
