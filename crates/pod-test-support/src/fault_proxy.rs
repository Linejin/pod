//! A loopback fault-injection proxy for remote-transport tests (docs/17 R4).
//!
//! [`FaultProxy`] listens on `127.0.0.1:0`. For every accepted connection it
//! opens one [`Upstream`] (for Pod: a `pod remote-stdio` child, like ssh does
//! on the far side) and relays bytes both ways through a bounded queue per
//! direction, adding:
//!
//! - **delay**: half the round-trip time on each direction (pipelined: every
//!   piece is due `rtt / 2` after it was read, not after the previous one),
//! - **throttle**: a per-connection, per-direction byte rate,
//! - **cuts**: [`FaultProxy::cut`] ends every live connection like a dropped
//!   network (client socket shut, upstream closed, i.e. the bridge dies the
//!   way it does when ssh drops) and refuses new ones until
//!   [`FaultProxy::restore`]; [`FaultProxy::drop_replies_then_cut`] first
//!   swallows everything coming back (a lost answer) and then cuts.
//!
//! Memory is bounded: a direction holds at most `buffer_limit` bytes; past
//! that the proxy stops reading, so the sender feels back-pressure. The
//! largest amount ever held is in [`Stats::max_buffered`].
//!
//! Loopback only, std only, for tests: nothing here is shipped.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The far side of one proxied connection.
pub struct Upstream {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Ends the far side at once (e.g. kill the child); called on a cut.
    pub close: Box<dyn FnMut() + Send>,
}

/// Opens one [`Upstream`] per accepted connection.
pub type UpstreamFn = dyn Fn() -> io::Result<Upstream> + Send + Sync;

/// Counters of one proxy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Connections relayed.
    pub links: usize,
    /// Connections refused while cut.
    pub refused: usize,
    /// Live connections ended by cuts.
    pub cut: usize,
    /// Bytes client → upstream and upstream → client delivered.
    pub up_bytes: u64,
    pub down_bytes: u64,
    /// Bytes swallowed by [`FaultProxy::drop_replies_then_cut`].
    pub dropped_bytes: u64,
    /// Largest number of bytes one direction ever held.
    pub max_buffered: usize,
}

struct Shared {
    rtt: Mutex<Duration>,
    throttle: Mutex<Option<u64>>,
    down: AtomicBool,
    stop: AtomicBool,
    buffer_limit: usize,
    links: Mutex<Vec<Arc<LinkState>>>,
    n_links: AtomicUsize,
    refused: AtomicUsize,
    cut: AtomicUsize,
    up_bytes: AtomicU64,
    down_bytes: AtomicU64,
    dropped_bytes: AtomicU64,
    max_buffered: AtomicUsize,
}

struct LinkState {
    client: TcpStream,
    close: Mutex<Option<Box<dyn FnMut() + Send>>>,
    closed: AtomicBool,
    /// Swallow upstream → client bytes from now on.
    blackhole: AtomicBool,
    queues: [Arc<Queue>; 2],
}

impl LinkState {
    fn end(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.client.shutdown(Shutdown::Both);
        if let Some(mut c) = self.close.lock().unwrap().take() {
            c();
        }
        for q in &self.queues {
            q.close();
        }
    }
}

/// One direction's pieces: (due time, bytes).
struct Queue {
    inner: Mutex<QueueInner>,
    cond: Condvar,
}

#[derive(Default)]
struct QueueInner {
    items: VecDeque<(Instant, Vec<u8>)>,
    bytes: usize,
    eof: bool,
    closed: bool,
}

impl Queue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(QueueInner::default()),
            cond: Condvar::new(),
        })
    }

    /// Wait for room, then queue. False when closed.
    fn push(&self, due: Instant, data: Vec<u8>, limit: usize, shared: &Shared) -> bool {
        let mut q = self.inner.lock().unwrap();
        while !q.closed && q.bytes > 0 && q.bytes + data.len() > limit {
            q = self.cond.wait(q).unwrap();
        }
        if q.closed {
            return false;
        }
        q.bytes += data.len();
        shared.max_buffered.fetch_max(q.bytes, Ordering::SeqCst);
        q.items.push_back((due, data));
        self.cond.notify_all();
        true
    }

    fn finish(&self) {
        self.inner.lock().unwrap().eof = true;
        self.cond.notify_all();
    }

    fn close(&self) {
        self.inner.lock().unwrap().closed = true;
        self.cond.notify_all();
    }

    /// The next piece; `None` at end (eof drained) or when closed.
    fn pop(&self) -> Option<(Instant, Vec<u8>)> {
        let mut q = self.inner.lock().unwrap();
        loop {
            if q.closed {
                return None;
            }
            if let Some(item) = q.items.pop_front() {
                q.bytes -= item.1.len();
                self.cond.notify_all();
                return Some(item);
            }
            if q.eof {
                return None;
            }
            q = self.cond.wait(q).unwrap();
        }
    }
}

/// See the module docs.
pub struct FaultProxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

/// Read size of the relay; also the throttle's write step.
const PIECE: usize = 16 * 1024;

impl FaultProxy {
    /// Listen on a free loopback port; `upstream` opens the far side of each
    /// connection. Default: no delay, no throttle, 256 KiB per direction.
    pub fn start(
        upstream: impl Fn() -> io::Result<Upstream> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        Self::with_buffer(upstream, 256 * 1024)
    }

    /// [`FaultProxy::start`] with another per-direction buffer limit.
    pub fn with_buffer(
        upstream: impl Fn() -> io::Result<Upstream> + Send + Sync + 'static,
        buffer_limit: usize,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            rtt: Mutex::new(Duration::ZERO),
            throttle: Mutex::new(None),
            down: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            buffer_limit: buffer_limit.max(PIECE),
            links: Mutex::new(Vec::new()),
            n_links: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            cut: AtomicUsize::new(0),
            up_bytes: AtomicU64::new(0),
            down_bytes: AtomicU64::new(0),
            dropped_bytes: AtomicU64::new(0),
            max_buffered: AtomicUsize::new(0),
        });
        let s = shared.clone();
        let upstream: Arc<UpstreamFn> = Arc::new(upstream);
        let accept = std::thread::spawn(move || accept_loop(listener, s, upstream));
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A client connection through the proxy.
    pub fn connect(&self) -> io::Result<TcpStream> {
        let s = TcpStream::connect(self.addr)?;
        s.set_nodelay(true)?;
        Ok(s)
    }

    /// Round-trip time added from now on (half each way).
    pub fn set_rtt(&self, rtt: Duration) {
        *self.shared.rtt.lock().unwrap() = rtt;
    }

    /// Bytes per second per direction and connection; `None`: unlimited.
    pub fn set_throttle(&self, bytes_per_sec: Option<u64>) {
        *self.shared.throttle.lock().unwrap() = bytes_per_sec.map(|b| b.max(1));
    }

    /// Drop the network: end every live connection, refuse new ones.
    pub fn cut(&self) {
        self.shared.down.store(true, Ordering::SeqCst);
        let links: Vec<Arc<LinkState>> = self.shared.links.lock().unwrap().drain(..).collect();
        for l in links {
            if !l.closed.load(Ordering::SeqCst) {
                self.shared.cut.fetch_add(1, Ordering::SeqCst);
            }
            l.end();
        }
    }

    /// Lose the answers: from now on nothing that comes back reaches the
    /// clients; after `after`, [`FaultProxy::cut`]. Returns at once.
    pub fn drop_replies_then_cut(&self, after: Duration) {
        for l in self.shared.links.lock().unwrap().iter() {
            l.blackhole.store(true, Ordering::SeqCst);
        }
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            cut_shared(&shared);
        });
    }

    /// Let connections through again.
    pub fn restore(&self) {
        self.shared.down.store(false, Ordering::SeqCst);
    }

    pub fn is_cut(&self) -> bool {
        self.shared.down.load(Ordering::SeqCst)
    }

    /// Connections relayed and still open.
    pub fn live(&self) -> usize {
        self.shared
            .links
            .lock()
            .unwrap()
            .iter()
            .filter(|l| !l.closed.load(Ordering::SeqCst))
            .count()
    }

    pub fn stats(&self) -> Stats {
        let s = &self.shared;
        Stats {
            links: s.n_links.load(Ordering::SeqCst),
            refused: s.refused.load(Ordering::SeqCst),
            cut: s.cut.load(Ordering::SeqCst),
            up_bytes: s.up_bytes.load(Ordering::SeqCst),
            down_bytes: s.down_bytes.load(Ordering::SeqCst),
            dropped_bytes: s.dropped_bytes.load(Ordering::SeqCst),
            max_buffered: s.max_buffered.load(Ordering::SeqCst),
        }
    }

    /// The per-direction buffer limit.
    pub fn buffer_limit(&self) -> usize {
        self.shared.buffer_limit
    }
}

fn cut_shared(shared: &Shared) {
    shared.down.store(true, Ordering::SeqCst);
    let links: Vec<Arc<LinkState>> = shared.links.lock().unwrap().drain(..).collect();
    for l in links {
        if !l.closed.load(Ordering::SeqCst) {
            shared.cut.fetch_add(1, Ordering::SeqCst);
        }
        l.end();
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        cut_shared(&self.shared);
        // Wake the accept loop.
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, upstream: Arc<UpstreamFn>) {
    for conn in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(client) = conn else { continue };
        if shared.down.load(Ordering::SeqCst) {
            shared.refused.fetch_add(1, Ordering::SeqCst);
            let _ = client.shutdown(Shutdown::Both);
            continue;
        }
        let _ = client.set_nodelay(true);
        let Ok(up) = upstream() else {
            let _ = client.shutdown(Shutdown::Both);
            continue;
        };
        if let Err(e) = spawn_link(client, up, &shared) {
            eprintln!("fault proxy: link failed: {e}");
        }
    }
}

fn spawn_link(client: TcpStream, up: Upstream, shared: &Arc<Shared>) -> io::Result<()> {
    let Upstream {
        reader: up_reader,
        writer: up_writer,
        close,
    } = up;
    let link = Arc::new(LinkState {
        client: client.try_clone()?,
        close: Mutex::new(Some(close)),
        closed: AtomicBool::new(false),
        blackhole: AtomicBool::new(false),
        queues: [Queue::new(), Queue::new()],
    });
    shared.n_links.fetch_add(1, Ordering::SeqCst);
    shared.links.lock().unwrap().push(link.clone());
    // Client → upstream.
    let up_q = link.queues[0].clone();
    pump_in(Box::new(client.try_clone()?), up_q.clone(), shared.clone());
    pump_out(up_writer, up_q, shared.clone(), link.clone(), Dir::Up);
    // Upstream → client.
    let down_q = link.queues[1].clone();
    pump_in(up_reader, down_q.clone(), shared.clone());
    pump_out(Box::new(client), down_q, shared.clone(), link, Dir::Down);
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
}

/// Read pieces and queue them, due `rtt / 2` from now.
fn pump_in(mut from: Box<dyn Read + Send>, q: Arc<Queue>, shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; PIECE];
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => {
                    q.finish();
                    return;
                }
                Ok(n) => {
                    let due = Instant::now() + *shared.rtt.lock().unwrap() / 2;
                    if !q.push(due, buf[..n].to_vec(), shared.buffer_limit, &shared) {
                        return;
                    }
                }
            }
        }
    });
}

/// Deliver queued pieces when due, at the throttled rate.
fn pump_out(
    mut to: Box<dyn Write + Send>,
    q: Arc<Queue>,
    shared: Arc<Shared>,
    link: Arc<LinkState>,
    dir: Dir,
) {
    std::thread::spawn(move || {
        let mut next_free = Instant::now();
        while let Some((due, data)) = q.pop() {
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            if dir == Dir::Down && link.blackhole.load(Ordering::SeqCst) {
                shared
                    .dropped_bytes
                    .fetch_add(data.len() as u64, Ordering::SeqCst);
                continue;
            }
            let rate = *shared.throttle.lock().unwrap();
            let step = match rate {
                Some(r) => ((r / 20) as usize).clamp(1, PIECE),
                None => data.len().max(1),
            };
            for piece in data.chunks(step) {
                if let Some(r) = rate {
                    let now = Instant::now();
                    if next_free > now {
                        std::thread::sleep(next_free - now);
                    }
                    next_free =
                        next_free.max(now) + Duration::from_secs_f64(piece.len() as f64 / r as f64);
                }
                if to.write_all(piece).and_then(|()| to.flush()).is_err() {
                    link.end();
                    return;
                }
                let counter = match dir {
                    Dir::Up => &shared.up_bytes,
                    Dir::Down => &shared.down_bytes,
                };
                counter.fetch_add(piece.len() as u64, Ordering::SeqCst);
            }
        }
        // The sending side ended: pass the end on, unless the link was cut.
        if !link.closed.load(Ordering::SeqCst) {
            match dir {
                // Closing the upstream's stdin lets a bridge drain and exit.
                Dir::Up => drop(to),
                Dir::Down => {
                    let _ = link.client.shutdown(Shutdown::Write);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An echo upstream in-process (two OS pipes and a thread).
    fn echo() -> io::Result<Upstream> {
        let (in_r, in_w) = io::pipe()?;
        let (out_r, out_w) = io::pipe()?;
        let closed = Arc::new(AtomicBool::new(false));
        let c = closed.clone();
        std::thread::spawn(move || {
            let mut r = in_r;
            let mut w = out_w;
            let mut buf = [0u8; 4096];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 || c.load(Ordering::SeqCst) || w.write_all(&buf[..n]).is_err() {
                    return;
                }
            }
        });
        Ok(Upstream {
            reader: Box::new(out_r),
            writer: Box::new(in_w),
            close: Box::new(move || closed.store(true, Ordering::SeqCst)),
        })
    }

    fn round_trip(s: &mut TcpStream, msg: &[u8]) -> Duration {
        let t = Instant::now();
        s.write_all(msg).unwrap();
        let mut got = vec![0u8; msg.len()];
        s.read_exact(&mut got).unwrap();
        assert_eq!(got, msg);
        t.elapsed()
    }

    #[test]
    fn relays_with_the_configured_round_trip() {
        let p = FaultProxy::start(echo).unwrap();
        let mut s = p.connect().unwrap();
        assert!(round_trip(&mut s, b"hi") < Duration::from_millis(100));
        p.set_rtt(Duration::from_millis(120));
        let rtt = round_trip(&mut s, b"again");
        assert!(rtt >= Duration::from_millis(115), "{rtt:?}");
        assert!(rtt < Duration::from_millis(600), "{rtt:?}");
        // Counted right after each write: allow the last one to land.
        let t = Instant::now();
        while p.stats().down_bytes < 7 && t.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let st = p.stats();
        assert_eq!((st.links, st.up_bytes, st.down_bytes), (1, 7, 7));
    }

    #[test]
    fn throttle_limits_the_rate_and_the_buffer_stays_bounded() {
        let p = FaultProxy::with_buffer(echo, 32 * 1024).unwrap();
        p.set_throttle(Some(200 * 1024));
        let mut s = p.connect().unwrap();
        let mut r = s.try_clone().unwrap();
        let reader = std::thread::spawn(move || {
            let mut got = vec![0u8; 200 * 1024];
            r.read_exact(&mut got).unwrap();
            got.len()
        });
        let t = Instant::now();
        s.write_all(&vec![7u8; 200 * 1024]).unwrap();
        assert_eq!(reader.join().unwrap(), 200 * 1024);
        // 200 KiB at 200 KiB/s per direction: about a second.
        assert!(
            t.elapsed() >= Duration::from_millis(800),
            "{:?}",
            t.elapsed()
        );
        assert!(
            p.stats().max_buffered <= 32 * 1024 + PIECE,
            "{:?}",
            p.stats()
        );
    }

    #[test]
    fn cut_ends_live_links_and_refuses_until_restored() {
        let p = FaultProxy::start(echo).unwrap();
        let mut s = p.connect().unwrap();
        round_trip(&mut s, b"x");
        p.cut();
        let mut b = [0u8; 1];
        assert!(matches!(s.read(&mut b), Ok(0) | Err(_)), "live link ended");
        let mut s2 = p.connect().unwrap();
        assert!(
            matches!(s2.read(&mut b), Ok(0) | Err(_)),
            "refused while cut"
        );
        p.restore();
        let mut s3 = p.connect().unwrap();
        round_trip(&mut s3, b"back");
        let st = p.stats();
        assert_eq!((st.cut, st.refused, st.links), (1, 1, 2));
    }

    #[test]
    fn dropped_replies_never_arrive() {
        let p = FaultProxy::start(echo).unwrap();
        let mut s = p.connect().unwrap();
        round_trip(&mut s, b"x");
        p.drop_replies_then_cut(Duration::from_millis(300));
        s.write_all(b"lost").unwrap();
        let mut b = [0u8; 4];
        assert!(matches!(s.read(&mut b), Ok(0) | Err(_)));
        assert_eq!(p.stats().dropped_bytes, 4);
        assert_eq!(p.stats().up_bytes, 5, "the request itself went through");
    }
}
