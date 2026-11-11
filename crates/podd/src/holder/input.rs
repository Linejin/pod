//! The child's input: one queue, written in order by one thread (never on
//! tokio). Daemon input is refused past a cap and acknowledged once written
//! (`InputAck`, holder protocol 3); replies to terminal queries, which the
//! holder writes itself, have their own budget ([`REPLY_BYTE_CAP`],
//! [`REPLY_COUNT_CAP`]).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
#[cfg(debug_assertions)]
use std::time::Duration;

use super::connection::Conn;
use super::process::Holder;
use super::{REPLY_BYTE_CAP, REPLY_COUNT_CAP};
use crate::holder_proto::Frame;

/// What the PTY input thread writes, in order.
pub(super) struct InputItem {
    bytes: Vec<u8>,
    /// The daemon connection it came from (acknowledged once written).
    from: Option<Arc<Conn>>,
}

/// The child's input queue, with byte accounting.
pub(super) struct InputQueue {
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

    pub(super) fn new(tx: std_mpsc::Sender<InputItem>) -> Self {
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
    pub(super) fn push_reply(&self, bytes: Vec<u8>) -> bool {
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
    pub(super) fn take_drop_notice(&self) -> Option<u64> {
        self.drop_notice
            .swap(false, Ordering::SeqCst)
            .then(|| self.replies_dropped.load(Ordering::SeqCst))
    }

    /// Daemon input: refused (false) when the queue is over `cap`.
    pub(super) fn push_from(&self, conn: &Arc<Conn>, bytes: Vec<u8>, cap: usize) -> bool {
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

impl Holder {
    /// Input bytes from `conn` were consumed: acknowledge them (protocol 3).
    pub(super) fn ack_input(&self, conn: &Arc<Conn>, n: usize) {
        let written = conn.consumed.fetch_add(n as u64, Ordering::SeqCst) + n as u64;
        if !conn.acks.load(Ordering::SeqCst) {
            return;
        }
        let mut st = self.state.lock().unwrap();
        if st.sub.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
            Holder::send(&mut st, &Frame::InputAck { written });
        }
    }
}

/// The thread that writes the child's input, in order, and acknowledges
/// daemon input once written. Ends when a write fails.
pub(super) fn spawn_writer(h: Arc<Holder>, input_rx: std_mpsc::Receiver<InputItem>) {
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

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::super::INPUT_HARD_CAP;
    use super::*;

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
}
