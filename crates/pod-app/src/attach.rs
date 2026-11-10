//! One `terminal.attach` connection per panel, on its own thread.
//!
//! Protocol (docs/03 §4.1): the reply carries an ANSI snapshot and the output
//! offset `seq` it corresponds to; then `terminal.output {seq, data}`
//! notifications follow with contiguous offsets. The thread checks the
//! offsets and re-attaches on any gap, on a lost connection (podd restarted:
//! terminals survive in their holders) and when the UI falls too far behind.
//! Every (re)attach starts with [`Chunk::Reset`], so the view is rebuilt from
//! the new snapshot instead of mixing two streams.
//!
//! The thread never starts podd itself; the control connection
//! ([`crate::source`]) does that. Dropping the [`AttachHandle`] detaches: it
//! cancels the blocking pipe read so idle terminals detach immediately too.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc;
use podd::protocol::{self as p, Method, codes};
use serde_json::Value;

/// Re-attach (fresh snapshot) instead of buffering more than this for the UI.
const MAX_PENDING: usize = 16 << 20;
const RETRY: Duration = Duration::from_millis(300);
/// Scrollback lines requested with each snapshot.
const SNAPSHOT_SCROLLBACK: usize = 2000;

/// Stream items for the UI, in order.
#[derive(Debug, PartialEq)]
pub enum Chunk {
    /// Start over from this snapshot (first attach and every re-attach).
    Reset {
        snapshot: Vec<u8>,
        info: Box<p::TerminalInfo>,
    },
    /// Raw output (consecutive outputs are merged).
    Output(Vec<u8>),
    Exited {
        code: Option<i32>,
        killed: bool,
    },
    /// Output was skipped (podd `terminal.lagged`, or the UI fell behind):
    /// a fresh snapshot follows. The panel tells the user briefly.
    Lagged,
    /// Connection to podd lost; retrying.
    Disconnected,
    /// The terminal no longer exists in podd.
    Gone,
}

#[derive(Default)]
struct Queue {
    chunks: VecDeque<Chunk>,
    output_bytes: usize,
}

struct Shared {
    queue: Mutex<Queue>,
    /// A wakeup is in flight (coalesce: one wakeup per UI drain).
    signaled: AtomicBool,
    wake: mpsc::UnboundedSender<()>,
    cancelled: AtomicBool,
    /// Raw handle of the live pipe, for cancelling a blocked read on detach.
    handle: Mutex<Option<usize>>,
    /// Current grid size of the view (used for each attach).
    size: Mutex<Option<(u16, u16)>>,
    /// The UI fell behind: drop the queue and re-attach.
    lagged: AtomicBool,
}

impl Shared {
    fn push(&self, chunk: Chunk) {
        {
            let mut q = self.queue.lock().unwrap();
            match chunk {
                Chunk::Output(bytes) => {
                    q.output_bytes += bytes.len();
                    if q.output_bytes > MAX_PENDING {
                        self.lagged.store(true, Ordering::Relaxed);
                    }
                    if let Some(Chunk::Output(last)) = q.chunks.back_mut() {
                        last.extend_from_slice(&bytes);
                    } else {
                        q.chunks.push_back(Chunk::Output(bytes));
                    }
                }
                Chunk::Reset { .. } => {
                    // Everything before a reset is obsolete.
                    q.chunks.clear();
                    q.output_bytes = 0;
                    q.chunks.push_back(chunk);
                }
                other => q.chunks.push_back(other),
            }
        }
        if !self.signaled.swap(true, Ordering::AcqRel) {
            let _ = self.wake.unbounded_send(());
        }
    }
}

/// Owner side of an attach thread. Drop = detach.
pub struct AttachHandle {
    shared: Arc<Shared>,
}

impl AttachHandle {
    /// Start attaching to `terminal_id`. `wake` gets one message per batch of
    /// new chunks; drain them with [`AttachHandle::take`].
    pub fn spawn(pipe: String, terminal_id: String, wake: mpsc::UnboundedSender<()>) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            signaled: AtomicBool::new(false),
            wake,
            cancelled: AtomicBool::new(false),
            handle: Mutex::new(None),
            size: Mutex::new(None),
            lagged: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let id = terminal_id.clone();
        std::thread::Builder::new()
            .name(format!("attach-{terminal_id}"))
            .spawn(move || run(pipe, id, thread_shared))
            .expect("spawn attach thread");
        Self { shared }
    }

    /// Everything queued since the last call.
    pub fn take(&self) -> Vec<Chunk> {
        self.shared.signaled.store(false, Ordering::Release);
        let mut q = self.shared.queue.lock().unwrap();
        q.output_bytes = 0;
        q.chunks.drain(..).collect()
    }

    /// Setter for the view's grid size; the next (re)attach asks podd for it
    /// so the snapshot fits the view.
    pub fn size_setter(&self) -> impl Fn(u16, u16) + 'static {
        let shared = self.shared.clone();
        move |cols, rows| *shared.size.lock().unwrap() = Some((cols, rows))
    }
}

impl Drop for AttachHandle {
    fn drop(&mut self) {
        self.shared.cancelled.store(true, Ordering::Release);
        cancel_io(&self.shared);
        // The thread may have been between two reads when we cancelled; keep
        // cancelling briefly until it let go of the pipe.
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            for _ in 0..40 {
                if shared.handle.lock().unwrap().is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
                cancel_io(&shared);
            }
        });
    }
}

fn cancel_io(shared: &Shared) {
    let guard = shared.handle.lock().unwrap();
    if let Some(h) = *guard {
        // SAFETY: the handle stays open while `handle` holds it (the attach
        // thread clears it under this lock before closing the file).
        unsafe {
            windows_sys::Win32::System::IO::CancelIoEx(
                h as windows_sys::Win32::Foundation::HANDLE,
                std::ptr::null(),
            );
        }
    }
}

enum Outcome {
    /// Try again after a short delay (connection lost, gap, lag).
    Retry { disconnected: bool },
    /// Stop: exited, gone, or detached.
    Stop,
}

fn run(pipe: String, id: String, shared: Arc<Shared>) {
    // Give the view one layout pass so the first snapshot has its size.
    for _ in 0..25 {
        if shared.size.lock().unwrap().is_some() || shared.cancelled.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut was_disconnected = false;
    while !shared.cancelled.load(Ordering::Acquire) {
        match attach_once(&pipe, &id, &shared) {
            Outcome::Stop => break,
            Outcome::Retry { disconnected } => {
                if disconnected && !was_disconnected && !shared.cancelled.load(Ordering::Acquire) {
                    shared.push(Chunk::Disconnected);
                }
                was_disconnected = disconnected;
                std::thread::sleep(RETRY);
            }
        }
    }
}

fn attach_once(pipe: &str, id: &str, shared: &Shared) -> Outcome {
    let file = match podd::transport::open_pipe(pipe) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Outcome::Retry { disconnected: true },
        Err(_) => return Outcome::Retry { disconnected: true },
    };
    *shared.handle.lock().unwrap() = Some(file.as_raw_handle() as usize);
    let outcome = stream(file_pair(file), id, shared);
    // `stream` consumed the file; clear the handle (it is closed now).
    *shared.handle.lock().unwrap() = None;
    outcome
}

struct Pair {
    reader: BufReader<File>,
    writer: File,
}

fn file_pair(file: File) -> Option<Pair> {
    let writer = file.try_clone().ok()?;
    Some(Pair {
        reader: BufReader::new(file),
        writer,
    })
}

fn stream(pair: Option<Pair>, id: &str, shared: &Shared) -> Outcome {
    let Some(Pair {
        mut reader,
        mut writer,
    }) = pair
    else {
        return Outcome::Retry { disconnected: true };
    };
    // Keep the handle registered until both halves are dropped, under the
    // lock, so a concurrent cancel never touches a closed handle.
    let close = |reader: BufReader<File>, writer: File| {
        let mut g = shared.handle.lock().unwrap();
        *g = None;
        drop(reader);
        drop(writer);
    };
    if shared.cancelled.load(Ordering::Acquire) {
        close(reader, writer);
        return Outcome::Stop;
    }
    let size = *shared.size.lock().unwrap();
    let params = p::AttachParams {
        target: p::TerminalRef::id(id),
        scrollback: Some(SNAPSHOT_SCROLLBACK),
        cols: size.map(|s| s.0),
        rows: size.map(|s| s.1),
    };
    let req = p::Request {
        jsonrpc: p::JSONRPC_VERSION.into(),
        id: Some(Value::from(1)),
        method: p::TerminalAttach::NAME.into(),
        params: serde_json::to_value(&params).unwrap_or_default(),
    };
    let mut line = serde_json::to_string(&req).unwrap_or_default();
    line.push('\n');
    if writer
        .write_all(line.as_bytes())
        .and_then(|_| writer.flush())
        .is_err()
    {
        close(reader, writer);
        return Outcome::Retry { disconnected: true };
    }
    let mut buf = String::new();
    match reader.read_line(&mut buf) {
        Ok(n) if n > 0 => {}
        _ => {
            close(reader, writer);
            return Outcome::Retry {
                disconnected: !shared.cancelled.load(Ordering::Acquire),
            };
        }
    }
    let resp: p::Response = match serde_json::from_str(&buf) {
        Ok(r) => r,
        Err(_) => {
            close(reader, writer);
            return Outcome::Retry {
                disconnected: false,
            };
        }
    };
    if let Some(err) = resp.error {
        close(reader, writer);
        if err.code == codes::TERMINAL_NOT_FOUND {
            shared.push(Chunk::Gone);
            return Outcome::Stop;
        }
        // A stopping daemon: the terminal lives on in its holder; reattach
        // to the next daemon and say we are disconnected meanwhile.
        return Outcome::Retry {
            disconnected: err.code == codes::SERVER_STOPPING,
        };
    }
    let result: p::AttachResult = match resp.result.map(serde_json::from_value) {
        Some(Ok(r)) => r,
        _ => {
            close(reader, writer);
            return Outcome::Retry {
                disconnected: false,
            };
        }
    };
    let snapshot = p::b64_decode(&result.snapshot).unwrap_or_default();
    let exited = result.terminal.state == p::TerminalState::Exited;
    let (code, killed) = (result.terminal.exit_code, result.terminal.killed);
    shared.lagged.store(false, Ordering::Relaxed);
    shared.push(Chunk::Reset {
        snapshot,
        info: Box::new(result.terminal),
    });
    if exited {
        // No more output can come; the snapshot is the final screen.
        close(reader, writer);
        shared.push(Chunk::Exited { code, killed });
        return Outcome::Stop;
    }

    let mut expect = result.seq;
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf);
        if shared.cancelled.load(Ordering::Acquire) {
            close(reader, writer);
            return Outcome::Stop;
        }
        match n {
            Ok(n) if n > 0 => {}
            _ => {
                close(reader, writer);
                return Outcome::Retry { disconnected: true };
            }
        }
        let Ok(note) = serde_json::from_str::<p::Notification>(&buf) else {
            continue;
        };
        match note.method.as_str() {
            "terminal.lagged" => {
                shared.push(Chunk::Lagged);
                close(reader, writer);
                return Outcome::Retry {
                    disconnected: false,
                };
            }
            p::NOTIFY_OUTPUT => {
                let Ok(o) = serde_json::from_value::<p::OutputEvent>(note.params) else {
                    continue;
                };
                if o.seq != expect {
                    // Gap or overlap: start over from a fresh snapshot.
                    close(reader, writer);
                    return Outcome::Retry {
                        disconnected: false,
                    };
                }
                let data = p::b64_decode(&o.data).unwrap_or_default();
                expect = o.seq + data.len() as u64;
                shared.push(Chunk::Output(data));
                if shared.lagged.load(Ordering::Relaxed) {
                    shared.push(Chunk::Lagged);
                    close(reader, writer);
                    return Outcome::Retry {
                        disconnected: false,
                    };
                }
            }
            p::NOTIFY_EXITED => {
                let e: Option<p::ExitedEvent> = serde_json::from_value(note.params).ok();
                close(reader, writer);
                shared.push(Chunk::Exited {
                    code: e.as_ref().and_then(|e| e.exit_code),
                    killed: e.is_some_and(|e| e.killed),
                });
                return Outcome::Stop;
            }
            // Size is owned by the view (last resize wins, docs/03 §4.1).
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> (Arc<Shared>, mpsc::UnboundedReceiver<()>) {
        let (tx, rx) = mpsc::unbounded();
        let s = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            signaled: AtomicBool::new(false),
            wake: tx,
            cancelled: AtomicBool::new(false),
            handle: Mutex::new(None),
            size: Mutex::new(None),
            lagged: AtomicBool::new(false),
        });
        (s, rx)
    }

    #[test]
    fn outputs_merge_and_reset_clears() {
        let (s, mut rx) = shared();
        s.push(Chunk::Output(b"ab".to_vec()));
        s.push(Chunk::Output(b"cd".to_vec()));
        // One wakeup for both.
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
        assert_eq!(s.queue.lock().unwrap().chunks.len(), 1);
        let info = p::TerminalInfo {
            id: "t1".into(),
            name: None,
            argv: vec![],
            cwd: String::new(),
            state: p::TerminalState::Running,
            exit_code: None,
            pid: None,
            holder_pid: None,
            cols: 80,
            rows: 24,
            created_ms: 0,
            exited_ms: None,
            killed: false,
            lost: false,
            attached: 1,
            conpty: None,
            status: None,
            ..Default::default()
        };
        s.push(Chunk::Reset {
            snapshot: b"S".to_vec(),
            info: Box::new(info),
        });
        s.push(Chunk::Output(b"e".to_vec()));
        let q = &s.queue.lock().unwrap().chunks;
        assert_eq!(q.len(), 2);
        assert!(matches!(q[0], Chunk::Reset { .. }));
        assert_eq!(q[1], Chunk::Output(b"e".to_vec()));
    }

    #[test]
    fn lag_flag() {
        let (s, _rx) = shared();
        s.push(Chunk::Output(vec![0; MAX_PENDING + 1]));
        assert!(s.lagged.load(Ordering::Relaxed));
    }
}
