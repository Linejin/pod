//! `pod-holder`: owns one terminal (ConPTY + Job Object) independently of the
//! daemon, so terminals survive `podd` stopping, crashing or restarting.
//!
//! Started detached by the daemon as `pod-holder --spec <file>`. It keeps an
//! authoritative VT state (to answer terminal queries and to give a
//! reconnecting daemon an exact snapshot) and serves the frame protocol in
//! [`crate::holder_proto`] on its own user-only named pipe. It exits only on
//! `Release` (after the child exited), which closes the Job Object.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;

use crate::holder_proto::{ExitInfo, Frame, HolderHello, HolderRecord, HolderSpec, read_frame};
use crate::pty::Pty;
use crate::vt::{OSC7501_REPLY, Osc7501Query, Vt};
use crate::winsec::UserOnly;

/// Largest raw output chunk read from the PTY at once (one `terminal.output`
/// notification carries at most this much before base64).
pub const OUTPUT_CHUNK: usize = 64 * 1024;

pub use crate::holder_proto::HOLDER_PROTOCOL;

/// Output bytes queued to the connected daemon and not yet written past
/// which the PTY reader waits (so the child blocks on its own output instead
/// of the holder buffering without limit). Control frames never wait.
pub const OUTPUT_BUDGET: usize = 8 * 1024 * 1024;
/// Input bytes waiting for the PTY past which daemon input is dropped
/// (logged, and still acknowledged). Daemons >= holder protocol 3 keep their
/// input in flight far below this through `InputAck`; this guards against
/// older daemons. Replies to terminal queries have their own budget below.
pub const INPUT_HARD_CAP: usize = 16 * 1024 * 1024;
/// Replies to terminal queries (VT/OSC answers the holder writes itself)
/// waiting for the PTY: past this many bytes or items new replies are
/// dropped (logged once per burst). A child that keeps asking without
/// reading its input otherwise grows the holder without limit.
pub const REPLY_BYTE_CAP: usize = 256 * 1024;
pub const REPLY_COUNT_CAP: usize = 4096;

/// Byte accounting of one daemon connection's outgoing queue.
#[derive(Default)]
struct OutBudget {
    queued: Mutex<usize>,
    room: Condvar,
    closed: AtomicBool,
}

impl OutBudget {
    fn add(&self, n: usize) {
        *self.queued.lock().unwrap() += n;
    }

    fn release(&self, n: usize) {
        let mut q = self.queued.lock().unwrap();
        *q = q.saturating_sub(n);
        self.room.notify_all();
    }

    /// The connection is gone: nobody waits for it any more.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _q = self.queued.lock().unwrap();
        self.room.notify_all();
    }

    /// Wait until `n` more bytes fit under `limit` (an empty queue always
    /// takes one chunk) or the connection closed: true. False after
    /// `timeout` with the queue still full.
    fn wait_room(&self, n: usize, limit: usize, timeout: Duration) -> bool {
        let q = self.queued.lock().unwrap();
        let (q, _) = self
            .room
            .wait_timeout_while(q, timeout, |q| {
                *q > 0 && *q + n > limit && !self.closed.load(Ordering::SeqCst)
            })
            .unwrap();
        *q == 0 || *q + n <= limit || self.closed.load(Ordering::SeqCst)
    }
}

/// One connected daemon.
struct Conn {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    budget: OutBudget,
    /// The daemon speaks holder protocol >= 3 (`InputAck`).
    acks: AtomicBool,
    /// Input bytes from this connection consumed so far (for `InputAck`).
    consumed: AtomicU64,
    /// Input from this connection is being dropped (logged once per episode).
    dropping: AtomicBool,
}

impl Conn {
    fn new(tx: mpsc::UnboundedSender<Vec<u8>>) -> Self {
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

/// What the PTY input thread writes, in order.
struct InputItem {
    bytes: Vec<u8>,
    /// The daemon connection it came from (acknowledged once written).
    from: Option<Arc<Conn>>,
}

/// The child's input queue, with byte accounting.
struct InputQueue {
    tx: Mutex<std_mpsc::Sender<InputItem>>,
    queued: AtomicUsize,
    /// Reply bytes / items queued and not yet written.
    reply_bytes: AtomicUsize,
    reply_items: AtomicUsize,
    /// Replies dropped over the reply budget, in total.
    replies_dropped: AtomicU64,
    /// Set by the first drop of a burst; taken (and logged) by the reader.
    drop_notice: AtomicBool,
    /// Currently dropping (cleared by the next accepted reply).
    reply_dropping: AtomicBool,
}

impl InputQueue {
    fn push(&self, item: InputItem) {
        self.queued.fetch_add(item.bytes.len(), Ordering::SeqCst);
        let _ = self.tx.lock().unwrap().send(item);
    }

    fn new(tx: std_mpsc::Sender<InputItem>) -> Self {
        Self {
            tx: Mutex::new(tx),
            queued: AtomicUsize::new(0),
            reply_bytes: AtomicUsize::new(0),
            reply_items: AtomicUsize::new(0),
            replies_dropped: AtomicU64::new(0),
            drop_notice: AtomicBool::new(false),
            reply_dropping: AtomicBool::new(false),
        }
    }

    /// Replies to terminal queries: queued within their own budget
    /// ([`REPLY_BYTE_CAP`], [`REPLY_COUNT_CAP`]); dropped (false) past it.
    fn push_reply(&self, bytes: Vec<u8>) -> bool {
        let n = bytes.len();
        let queued = self.reply_bytes.load(Ordering::SeqCst);
        let items = self.reply_items.load(Ordering::SeqCst);
        if items > 0 && (queued + n > REPLY_BYTE_CAP || items >= REPLY_COUNT_CAP) {
            self.replies_dropped.fetch_add(1, Ordering::SeqCst);
            if !self.reply_dropping.swap(true, Ordering::SeqCst) {
                self.drop_notice.store(true, Ordering::SeqCst);
            }
            return false;
        }
        self.reply_dropping.store(false, Ordering::SeqCst);
        self.reply_bytes.fetch_add(n, Ordering::SeqCst);
        self.reply_items.fetch_add(1, Ordering::SeqCst);
        self.push(InputItem { bytes, from: None });
        true
    }

    /// The writer wrote (or gave up on) an item.
    fn written(&self, item: &InputItem) {
        let n = item.bytes.len();
        self.queued.fetch_sub(n, Ordering::SeqCst);
        if item.from.is_none() {
            self.reply_bytes.fetch_sub(n, Ordering::SeqCst);
            self.reply_items.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// The total dropped so far, once per burst of drops (for the log).
    fn take_drop_notice(&self) -> Option<u64> {
        self.drop_notice
            .swap(false, Ordering::SeqCst)
            .then(|| self.replies_dropped.load(Ordering::SeqCst))
    }

    /// Daemon input: refused (false) when the queue is over `cap`.
    fn push_from(&self, conn: &Arc<Conn>, bytes: Vec<u8>, cap: usize) -> bool {
        let queued = self.queued.load(Ordering::SeqCst);
        if queued > 0 && queued + bytes.len() > cap {
            return false;
        }
        self.push(InputItem {
            bytes,
            from: Some(conn.clone()),
        });
        true
    }
}

struct State {
    vt: Vt,
    exited: Option<ExitInfo>,
    killed: bool,
    /// The connected daemon.
    sub: Option<Arc<Conn>>,
}

struct Holder {
    spec: HolderSpec,
    pty: Arc<Pty>,
    state: Mutex<State>,
    exit_code: Mutex<Option<i32>>,
    last_output: Mutex<Instant>,
    input: Arc<InputQueue>,
    log: Mutex<Option<File>>,
    sa: UserOnly,
}

impl Holder {
    fn log(&self, msg: &str) {
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "[pod-holder {} {}] {msg}", self.spec.id, crate::now_ms());
        }
    }

    fn send(state: &mut State, frame: &Frame) {
        if let Some(c) = &state.sub
            && !c.push(frame.encode())
        {
            state.sub = None;
        }
    }

    /// Before feeding `n` more output bytes: wait while the connected
    /// daemon's queue is over [`OUTPUT_BUDGET`] (backpressure to the child).
    fn wait_output_room(&self, n: usize) {
        loop {
            let Some(conn) = self.state.lock().unwrap().sub.clone() else {
                return;
            };
            if conn
                .budget
                .wait_room(n, OUTPUT_BUDGET, Duration::from_millis(250))
            {
                return;
            }
            // Still full: look again (the daemon may have been replaced).
        }
    }

    /// Input bytes from `conn` were consumed: acknowledge them (protocol 3).
    fn ack_input(&self, conn: &Arc<Conn>, n: usize) {
        let written = conn.consumed.fetch_add(n as u64, Ordering::SeqCst) + n as u64;
        if !conn.acks.load(Ordering::SeqCst) {
            return;
        }
        let mut st = self.state.lock().unwrap();
        if st.sub.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
            Holder::send(&mut st, &Frame::InputAck { written });
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

    fn record_path(&self) -> PathBuf {
        PathBuf::from(&self.spec.record_path)
    }
}

fn sibling(record: &Path, ext: &str) -> PathBuf {
    record.with_extension(ext)
}

/// Entry point of the `pod-holder` binary. Returns the process exit code.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let spec_path = match args
        .iter()
        .position(|a| a == "--spec")
        .and_then(|i| args.get(i + 1))
    {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: pod-holder --spec <file>   (started by podd)");
            return 2;
        }
    };
    let spec: HolderSpec = match std::fs::read(&spec_path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pod-holder: cannot read spec {}: {e}", spec_path.display());
            return 2;
        }
    };
    let _ = std::fs::remove_file(&spec_path);
    let record = PathBuf::from(&spec.record_path);
    let fail = |msg: String| {
        let _ = std::fs::write(sibling(&record, "err"), &msg);
        1
    };

    let sa = match UserOnly::new() {
        Ok(sa) => sa,
        Err(e) => return fail(format!("security descriptor: {e}")),
    };
    let (pty, mut output) = match Pty::spawn(
        &spec.argv,
        Path::new(&spec.cwd),
        &spec.env,
        spec.cols,
        spec.rows,
    ) {
        Ok(p) => p,
        Err(e) => return fail(format!("{e}")),
    };
    let pty = Arc::new(pty);
    // Tests only (debug builds): a holder that is slow to serve its pipe,
    // so a stop can land deterministically inside `terminal.run`.
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("POD_HOLDER_TEST_START_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_millis(ms.min(30_000)));
    }

    // All writes to the child's input go through one thread (never block
    // tokio); it starts once the holder exists (it acknowledges input).
    let (input_tx, input_rx) = std_mpsc::channel::<InputItem>();
    let input = Arc::new(InputQueue::new(input_tx));
    let reply: crate::vt::ReplySink = {
        let input = input.clone();
        Arc::new(move |b: &[u8]| {
            input.push_reply(b.to_vec());
        })
    };

    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(sibling(&record, "log"))
        .ok();
    let holder = Arc::new(Holder {
        state: Mutex::new(State {
            vt: Vt::new(spec.cols, spec.rows, Some(reply)),
            exited: None,
            killed: false,
            sub: None,
        }),
        spec,
        pty,
        exit_code: Mutex::new(None),
        last_output: Mutex::new(Instant::now()),
        input,
        log: Mutex::new(log),
        sa,
    });
    {
        let h = holder.clone();
        // Debug-only test knob: the writer never writes, like a child that
        // never reads its input (daemon-security-02).
        #[cfg(debug_assertions)]
        let stall_input = std::env::var_os("POD_HOLDER_TEST_STALL_INPUT").is_some();
        std::thread::spawn(move || {
            for item in input_rx {
                let n = item.bytes.len();
                #[cfg(debug_assertions)]
                if stall_input {
                    std::thread::sleep(Duration::from_secs(3600));
                }
                let ok = h.pty.write(&item.bytes).is_ok();
                h.input.written(&item);
                if let Some(conn) = &item.from {
                    h.ack_input(conn, n);
                }
                if !ok {
                    break;
                }
            }
        });
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            holder.pty.kill_tree(1);
            return fail(format!("runtime: {e}"));
        }
    };
    let first = {
        let _guard = rt.enter();
        // SAFETY: the attributes outlive the call.
        match unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(&holder.spec.pipe, holder.sa.as_ptr())
        } {
            Ok(s) => s,
            Err(e) => {
                holder.pty.kill_tree(1);
                return fail(format!(
                    "cannot create holder pipe {}: {e}",
                    holder.spec.pipe
                ));
            }
        }
    };

    let rec = HolderRecord {
        id: holder.spec.id.clone(),
        name: holder.spec.name.clone(),
        pipe: holder.spec.pipe.clone(),
        holder_pid: std::process::id(),
        child_pid: holder.pty.pid(),
        created_ms: holder.spec.created_ms,
    };
    let tmp = sibling(&record, "json.tmp");
    if let Err(e) = std::fs::write(&tmp, serde_json::to_vec_pretty(&rec).unwrap())
        .and_then(|_| std::fs::rename(&tmp, &record))
    {
        holder.pty.kill_tree(1);
        return fail(format!("cannot write record {}: {e}", record.display()));
    }
    holder.log(&format!(
        "started {:?} pid {} on {}",
        holder.spec.argv,
        holder.pty.pid(),
        holder.spec.pipe
    ));

    // PTY output reader.
    {
        let h = holder.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; OUTPUT_CHUNK];
            let mut osc7501 = Osc7501Query::default();
            loop {
                match output.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        h.wait_output_room(n);
                        *h.last_output.lock().unwrap() = Instant::now();
                        let chunk = &buf[..n];
                        let mut st = h.state.lock().unwrap();
                        let seq = st.vt.seq();
                        // Feed up to each OSC 7501 query and answer it right
                        // there, so replies leave in stream order (before the
                        // DA1 reply that Claude uses as its "no answer" fence).
                        let mut done = 0;
                        for end in osc7501.scan(chunk) {
                            st.vt.feed(&chunk[done..end]);
                            h.input.push_reply(OSC7501_REPLY.to_vec());
                            done = end;
                        }
                        st.vt.feed(&chunk[done..]);
                        if let Some(total) = h.input.take_drop_notice() {
                            h.log(&format!(
                                "terminal query replies dropped: over {REPLY_BYTE_CAP} bytes /                                  {REPLY_COUNT_CAP} replies waiting for the program ({total} in total)"
                            ));
                        }
                        Holder::send(
                            &mut st,
                            &Frame::Output {
                                seq,
                                data: chunk.to_vec(),
                            },
                        );
                    }
                }
            }
            // End of output: the pseudo console was closed after the child exited.
            let mut st = h.state.lock().unwrap();
            let info = ExitInfo {
                seq: st.vt.seq(),
                exit_code: *h.exit_code.lock().unwrap(),
                killed: st.killed,
                exited_ms: crate::now_ms(),
            };
            st.exited = Some(info.clone());
            Holder::send(&mut st, &Frame::Exited(info.clone()));
            drop(st);
            h.log(&format!(
                "exited code {:?} killed {}",
                info.exit_code, info.killed
            ));
        });
    }

    // Child exit watcher: let the last output drain, then close the console
    // so the reader sees end-of-file.
    {
        let h = holder.clone();
        std::thread::spawn(move || {
            let code = h.pty.wait();
            *h.exit_code.lock().unwrap() = code;
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3)
                && h.last_output.lock().unwrap().elapsed() < Duration::from_millis(150)
            {
                std::thread::sleep(Duration::from_millis(30));
            }
            h.pty.close_console();
        });
    }

    rt.block_on(accept_loop(holder, first));
    0
}

async fn accept_loop(holder: Arc<Holder>, mut server: NamedPipeServer) {
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
    use super::*;

    /// daemon-01: the output queue waits for room past its budget, takes one
    /// chunk when empty, and wakes on release or when the daemon is gone.
    #[test]
    fn output_budget_waits_for_room() {
        let b = Arc::new(OutBudget::default());
        let short = Duration::from_millis(30);
        // An empty queue always takes a chunk, even one over the limit.
        assert!(b.wait_room(500, 100, short));
        b.add(80);
        assert!(b.wait_room(20, 100, short));
        assert!(!b.wait_room(21, 100, short), "over the budget: wait");
        let b2 = b.clone();
        let t = std::thread::spawn(move || b2.wait_room(50, 100, Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(50));
        b.release(60);
        assert!(t.join().unwrap(), "a release wakes the waiter");
        b.add(1000);
        let b2 = b.clone();
        let t = std::thread::spawn(move || b2.wait_room(50, 100, Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(50));
        b.close();
        assert!(
            t.join().unwrap(),
            "a closed connection never holds the reader"
        );
    }

    /// daemon-01: daemon input past the hard cap is refused; replies to
    /// terminal queries are queued on their own budget.
    #[test]
    fn input_queue_caps_daemon_input_but_not_replies() {
        let (tx, rx) = std_mpsc::channel();
        let q = InputQueue::new(tx);
        let (ctx, _crx) = mpsc::unbounded_channel();
        let conn = Arc::new(Conn::new(ctx));
        // An empty queue takes one item even over the cap.
        assert!(q.push_from(&conn, vec![0; 150], 100));
        assert!(!q.push_from(&conn, vec![0; 1], 100), "over the cap");
        q.push_reply(b"\x1b[?1;2c".to_vec());
        assert_eq!(q.queued.load(Ordering::SeqCst), 157);
        let items: Vec<InputItem> = rx.try_iter().collect();
        assert_eq!(items.len(), 2);
        assert!(items[0].from.is_some() && items[1].from.is_none());
    }

    /// daemon-security-02: replies nobody writes (the child never reads its
    /// input) stop at their byte and count budget; the drop is noted once per
    /// burst, and replies are accepted again once the writer catches up.
    #[test]
    fn replies_have_their_own_budget() {
        let (tx, rx) = std_mpsc::channel();
        let q = InputQueue::new(tx);
        // Count cap: tiny replies.
        for _ in 0..REPLY_COUNT_CAP {
            assert!(q.push_reply(b"[0n".to_vec()));
        }
        assert!(!q.push_reply(b"[0n".to_vec()), "over the count cap");
        assert!(!q.push_reply(b"[0n".to_vec()));
        assert_eq!(q.take_drop_notice(), Some(2));
        assert_eq!(q.take_drop_notice(), None, "noted once per burst");
        // Daemon input is still governed by its own cap only.
        let (ctx, _crx) = mpsc::unbounded_channel();
        let conn = Arc::new(Conn::new(ctx));
        assert!(q.push_from(&conn, vec![0; 10], INPUT_HARD_CAP));
        // The writer catches up: replies are accepted again.
        let items: Vec<InputItem> = rx.try_iter().collect();
        assert_eq!(items.len(), REPLY_COUNT_CAP + 1);
        for item in &items {
            q.written(item);
        }
        assert_eq!(q.reply_items.load(Ordering::SeqCst), 0);
        assert_eq!(q.queued.load(Ordering::SeqCst), 0);
        assert!(q.push_reply(b"[0n".to_vec()));
        // Byte cap: large replies (one always fits an empty queue).
        let big = vec![b'x'; REPLY_BYTE_CAP / 2 + 1];
        assert!(q.push_reply(big.clone()));
        assert!(!q.push_reply(big), "over the byte cap");
        assert_eq!(q.take_drop_notice(), Some(3));
        assert!(q.reply_bytes.load(Ordering::SeqCst) <= REPLY_BYTE_CAP);
    }

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
