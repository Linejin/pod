//! Frame / paint counters (same method as pod-spike, docs/02 §6).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static FRAMES: AtomicU64 = AtomicU64::new(0);
static PAINTS: AtomicU64 = AtomicU64::new(0);
static PAINT_NS: AtomicU64 = AtomicU64::new(0);
static PAINT_MAX_NS: AtomicU64 = AtomicU64::new(0);
// Whole-frame CPU: from root render start to the last terminal paint end.
static FRAME_START_NS: AtomicU64 = AtomicU64::new(0);
static LAST_PAINT_END_NS: AtomicU64 = AtomicU64::new(0);
static FRAME_CPU_NS: AtomicU64 = AtomicU64::new(0);
static FRAME_CPU_MAX_NS: AtomicU64 = AtomicU64::new(0);
static SLOW_FRAMES: AtomicU64 = AtomicU64::new(0);

fn now_ns() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

pub fn record_frame() {
    let now = now_ns();
    let start = FRAME_START_NS.swap(now, Ordering::Relaxed);
    let end = LAST_PAINT_END_NS.load(Ordering::Relaxed);
    if start > 0 && end > start {
        let cpu = end - start;
        FRAME_CPU_NS.fetch_add(cpu, Ordering::Relaxed);
        FRAME_CPU_MAX_NS.fetch_max(cpu, Ordering::Relaxed);
        if cpu > 16_600_000 {
            SLOW_FRAMES.fetch_add(1, Ordering::Relaxed);
        }
    }
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_paint(d: Duration) {
    let ns = d.as_nanos() as u64;
    PAINTS.fetch_add(1, Ordering::Relaxed);
    PAINT_NS.fetch_add(ns, Ordering::Relaxed);
    PAINT_MAX_NS.fetch_max(ns, Ordering::Relaxed);
    LAST_PAINT_END_NS.store(now_ns(), Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Window {
    pub fps: f64,
    pub paints_per_sec: f64,
    pub avg_paint_ms: f64,
    pub max_paint_ms: f64,
    /// Terminal paints per rendered frame (how many terminals actually repainted).
    pub paints_per_frame: f64,
    /// Total terminal paint CPU per rendered frame.
    pub paint_ms_per_frame: f64,
    pub avg_frame_cpu_ms: f64,
    pub max_frame_cpu_ms: f64,
    pub slow_frames: u64,
}

/// Read and reset counters accumulated over `elapsed`.
pub fn take(elapsed: Duration) -> Window {
    let secs = elapsed.as_secs_f64().max(1e-6);
    let frames = FRAMES.swap(0, Ordering::Relaxed);
    let paints = PAINTS.swap(0, Ordering::Relaxed);
    let ns = PAINT_NS.swap(0, Ordering::Relaxed);
    let max = PAINT_MAX_NS.swap(0, Ordering::Relaxed);
    let fcpu = FRAME_CPU_NS.swap(0, Ordering::Relaxed);
    let fmax = FRAME_CPU_MAX_NS.swap(0, Ordering::Relaxed);
    let slow = SLOW_FRAMES.swap(0, Ordering::Relaxed);
    Window {
        fps: frames as f64 / secs,
        paints_per_sec: paints as f64 / secs,
        avg_paint_ms: if paints > 0 {
            ns as f64 / paints as f64 / 1e6
        } else {
            0.0
        },
        max_paint_ms: max as f64 / 1e6,
        paints_per_frame: if frames > 0 {
            paints as f64 / frames as f64
        } else {
            0.0
        },
        paint_ms_per_frame: if frames > 0 {
            ns as f64 / frames as f64 / 1e6
        } else {
            0.0
        },
        avg_frame_cpu_ms: if frames > 0 {
            fcpu as f64 / frames as f64 / 1e6
        } else {
            0.0
        },
        max_frame_cpu_ms: fmax as f64 / 1e6,
        slow_frames: slow,
    }
}
