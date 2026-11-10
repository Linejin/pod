//! Optional paint timing hook (pod-spike's FPS/paint statistics).

use std::sync::OnceLock;
use std::time::Duration;

static PAINT_HOOK: OnceLock<fn(Duration)> = OnceLock::new();

/// Install a callback invoked after every terminal element paint with its
/// CPU duration. Only the first call takes effect.
pub fn set_paint_hook(hook: fn(Duration)) {
    let _ = PAINT_HOOK.set(hook);
}

pub(crate) fn record_paint(d: Duration) {
    if let Some(h) = PAINT_HOOK.get() {
        h(d);
    }
}
