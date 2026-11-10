//! Frame counters, measured the same way as pod-spike (docs/02 §6):
//! fps = root `render` calls per second (GPUI redraws the whole window each
//! frame), frame CPU = root render start → end of the last paint in the frame
//! (a zero-size marker element painted after everything else).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::*;

static FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAME_START_NS: AtomicU64 = AtomicU64::new(0);
static LAST_PAINT_END_NS: AtomicU64 = AtomicU64::new(0);
static FRAME_CPU_NS: AtomicU64 = AtomicU64::new(0);
static FRAME_CPU_MAX_NS: AtomicU64 = AtomicU64::new(0);
static SLOW_FRAMES: AtomicU64 = AtomicU64::new(0);
static FIRST_PAINT_NS: AtomicU64 = AtomicU64::new(0);

pub fn process_start() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

fn now_ns() -> u64 {
    process_start().elapsed().as_nanos() as u64
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

fn record_paint_end() {
    let now = now_ns();
    LAST_PAINT_END_NS.store(now, Ordering::Relaxed);
    let _ = FIRST_PAINT_NS.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
}

/// Time from process start to the end of the first painted frame.
pub fn first_paint() -> Option<Duration> {
    match FIRST_PAINT_NS.load(Ordering::Relaxed) {
        0 => None,
        ns => Some(Duration::from_nanos(ns)),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub fps: f64,
    pub avg_frame_cpu_ms: f64,
    pub max_frame_cpu_ms: f64,
    pub slow_frames: u64,
}

pub fn take(elapsed: Duration) -> Sample {
    let secs = elapsed.as_secs_f64().max(1e-6);
    let frames = FRAMES.swap(0, Ordering::Relaxed);
    let fcpu = FRAME_CPU_NS.swap(0, Ordering::Relaxed);
    let fmax = FRAME_CPU_MAX_NS.swap(0, Ordering::Relaxed);
    let slow = SLOW_FRAMES.swap(0, Ordering::Relaxed);
    Sample {
        fps: frames as f64 / secs,
        avg_frame_cpu_ms: if frames > 0 {
            fcpu as f64 / frames as f64 / 1e6
        } else {
            0.0
        },
        max_frame_cpu_ms: fmax as f64 / 1e6,
        slow_frames: slow,
    }
}

pub fn summarize(label: &str, samples: &[Sample]) {
    if samples.is_empty() {
        println!("[summary] {label}: no samples");
        return;
    }
    let n = samples.len() as f64;
    let avg_fps = samples.iter().map(|s| s.fps).sum::<f64>() / n;
    let min_fps = samples.iter().map(|s| s.fps).fold(f64::MAX, f64::min);
    let avg_frame = samples.iter().map(|s| s.avg_frame_cpu_ms).sum::<f64>() / n;
    let max_frame = samples
        .iter()
        .map(|s| s.max_frame_cpu_ms)
        .fold(0.0, f64::max);
    let slow: u64 = samples.iter().map(|s| s.slow_frames).sum();
    println!(
        "[summary] {label}: samples={} fps avg={avg_fps:.1} min={min_fps:.1} | frame cpu avg={avg_frame:.2}ms max={max_frame:.2}ms slow(>16.6ms)={slow}",
        samples.len()
    );
}

/// Zero-size element placed last in the root so its paint ends the frame.
pub struct FrameEndMarker;

impl IntoElement for FrameEndMarker {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for FrameEndMarker {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: Position::Absolute,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
        record_paint_end();
    }
}
