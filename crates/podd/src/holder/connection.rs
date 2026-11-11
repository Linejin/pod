//! The holder's pipe server: one daemon at a time. A new connection gets
//! `Hello`, `Snapshot` and (if the child already exited) `Exited`, atomically
//! with respect to output, and replaces the previous daemon, which is how a
//! restarted daemon takes over. From the daemon: input, resize, kill,
//! release and its hello.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;

use super::budget::OutBudget;
use super::process::{Holder, State, sibling};
use super::{HOLDER_PROTOCOL, INPUT_HARD_CAP};
use crate::holder_proto::{Frame, HolderHello, read_frame};

/// One connected daemon.
pub(super) struct Conn {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(super) budget: OutBudget,
    /// The daemon speaks holder protocol >= 3 (`InputAck`).
    pub(super) acks: AtomicBool,
    /// Input bytes from this connection consumed so far (for `InputAck`).
    pub(super) consumed: AtomicU64,
    /// Input from this connection is being dropped (logged once per episode).
    dropping: AtomicBool,
}

impl Conn {
    pub(super) fn new(tx: mpsc::UnboundedSender<Vec<u8>>) -> Self {
        Self {
            tx,
            budget: OutBudget::default(),
            acks: AtomicBool::new(false),
            consumed: AtomicU64::new(0),
            dropping: AtomicBool::new(false),
        }
    }

    /// Queue an encoded frame (never waits). False when the connection is gone.
    fn push(&self, bytes: Vec<u8>) -> bool {
        let n = bytes.len();
        self.budget.add(n);
        if self.tx.send(bytes).is_err() {
            self.budget.release(n);
            return false;
        }
        true
    }
}

impl Holder {
    pub(super) fn send(state: &mut State, frame: &Frame) {
        if let Some(c) = &state.sub
            && !c.push(frame.encode())
        {
            state.sub = None;
        }
    }

    fn hello(&self, state: &State) -> HolderHello {
        let (cols, rows) = state.vt.size();
        HolderHello {
            version: HOLDER_PROTOCOL,
            id: self.spec.id.clone(),
            name: self.spec.name.clone(),
            argv: self.spec.argv.clone(),
            cwd: self.spec.cwd.clone(),
            created_ms: self.spec.created_ms,
            holder_pid: std::process::id(),
            child_pid: self.pty.pid(),
            cols,
            rows,
            conpty: crate::pty::conpty_api().kind.clone(),
            parent_terminal: self.spec.parent_terminal.clone(),
            parent_task: self.spec.parent_task.clone(),
        }
    }
}

/// The first instance of the holder's pipe (fails when the name is taken).
/// Inside the runtime.
pub(super) fn first_instance(h: &Holder) -> io::Result<NamedPipeServer> {
    // SAFETY: the attributes outlive the call.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(&h.spec.pipe, h.sa.as_ptr())
    }
}

pub(super) async fn accept_loop(holder: Arc<Holder>, mut server: NamedPipeServer) {
    loop {
        if let Err(e) = server.connect().await {
            holder.log(&format!("accept failed: {e}"));
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // SAFETY: the attributes outlive the call.
        let next = match unsafe {
            ServerOptions::new()
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(&holder.spec.pipe, holder.sa.as_ptr())
        } {
            Ok(n) => n,
            Err(e) => {
                holder.log(&format!("cannot create next pipe instance: {e}"));
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let conn = std::mem::replace(&mut server, next);
        tokio::spawn(serve_daemon(holder.clone(), conn));
    }
}

async fn serve_daemon(h: Arc<Holder>, pipe: NamedPipeServer) {
    let (mut rd, mut wr) = tokio::io::split(pipe);
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let conn = Arc::new(Conn::new(tx));
    {
        // Hello + snapshot + (exit) + subscription, atomically w.r.t. output.
        let mut st = h.state.lock().unwrap();
        let hello = h.hello(&st);
        conn.push(Frame::Hello(hello).encode());
        conn.push(
            Frame::Snapshot {
                seq: st.vt.seq(),
                data: st.vt.snapshot_ansi(usize::MAX),
            }
            .encode(),
        );
        if let Some(e) = &st.exited {
            conn.push(Frame::Exited(e.clone()).encode());
        }
        // Replaces (and so disconnects) any previous daemon.
        if let Some(old) = st.sub.replace(conn.clone()) {
            old.budget.close();
        }
    }
    h.log("daemon connected");
    let writer = {
        let conn = conn.clone();
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                if wr.write_all(&bytes).await.is_err() {
                    break;
                }
                conn.budget.release(bytes.len());
            }
        })
    };

    loop {
        let frame = match read_frame(&mut rd).await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                h.log(&format!("daemon read error: {e}"));
                break;
            }
        };
        match frame {
            Frame::Input(bytes) => {
                let n = bytes.len();
                if h.input.push_from(&conn, bytes, INPUT_HARD_CAP) {
                    if conn.dropping.swap(false, Ordering::SeqCst) {
                        h.log("input accepted again");
                    }
                } else {
                    if !conn.dropping.swap(true, Ordering::SeqCst) {
                        h.log(&format!(
                            "input dropped: over {INPUT_HARD_CAP} bytes wait for the program"
                        ));
                    }
                    // Consumed, as far as the daemon's budget is concerned.
                    h.ack_input(&conn, n);
                }
            }
            Frame::Resize { cols, rows } => {
                let (cols, rows) = (cols.clamp(2, 1000), rows.clamp(2, 500));
                if let Err(e) = h.pty.resize(cols, rows) {
                    h.log(&format!("resize failed: {e}"));
                }
                let mut st = h.state.lock().unwrap();
                st.vt.resize(cols, rows);
                let seq = st.vt.seq();
                Holder::send(&mut st, &Frame::Resized { seq, cols, rows });
            }
            Frame::Kill => {
                h.state.lock().unwrap().killed = true;
                h.log("kill (job)");
                h.pty.kill_tree(1);
            }
            Frame::Release => {
                if h.state.lock().unwrap().exited.is_some() {
                    h.log("released");
                    let record = h.record_path();
                    let _ = std::fs::remove_file(&record);
                    let _ = std::fs::remove_file(sibling(&record, "log"));
                    std::process::exit(0);
                }
                h.log("release ignored: child still running");
            }
            Frame::DaemonHello(d) => {
                // Protocol 3: input acknowledgements (`InputAck`).
                if d.version >= 3 {
                    conn.acks.store(true, Ordering::SeqCst);
                }
                h.log(&format!(
                    "daemon speaks holder protocol {} (this holder: {HOLDER_PROTOCOL})",
                    d.version
                ));
            }
            other => h.log(&format!("unexpected frame from daemon: {other:?}")),
        }
    }
    h.log("daemon disconnected");
    writer.abort();
    conn.budget.close();
    let mut st = h.state.lock().unwrap();
    if st.sub.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn)) {
        st.sub = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::OUTPUT_BUDGET;
    use super::*;

    /// Control frames are queued whatever the budget says.
    #[test]
    fn control_frames_never_wait() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let conn = Conn::new(tx);
        conn.budget.add(OUTPUT_BUDGET * 4);
        let resized = Frame::Resized {
            seq: 1,
            cols: 80,
            rows: 24,
        }
        .encode();
        assert!(conn.push(resized.clone()));
        assert!(conn.push(Frame::InputAck { written: 9 }.encode()));
        assert_eq!(rx.try_recv().unwrap(), resized);
    }
}
