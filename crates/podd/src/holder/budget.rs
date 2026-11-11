//! Output budget: bytes queued to the connected daemon and not yet written
//! to its pipe. Past [`OUTPUT_BUDGET`] the PTY reader waits, so the child
//! blocks on its own output instead of the holder buffering without limit.
//! Control frames are queued whatever the budget says (they never wait).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use super::OUTPUT_BUDGET;
use super::process::Holder;

/// Byte accounting of one daemon connection's outgoing queue.
#[derive(Default)]
pub(super) struct OutBudget {
    queued: Mutex<usize>,
    room: Condvar,
    closed: AtomicBool,
}

impl OutBudget {
    pub(super) fn add(&self, n: usize) {
        *self.queued.lock().unwrap() += n;
    }

    pub(super) fn release(&self, n: usize) {
        let mut q = self.queued.lock().unwrap();
        *q = q.saturating_sub(n);
        self.room.notify_all();
    }

    /// The connection is gone: nobody waits for it any more.
    pub(super) fn close(&self) {
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

impl Holder {
    /// Before feeding `n` more output bytes: wait while the connected
    /// daemon's queue is over [`OUTPUT_BUDGET`] (backpressure to the child).
    pub(super) fn wait_output_room(&self, n: usize) {
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
}
