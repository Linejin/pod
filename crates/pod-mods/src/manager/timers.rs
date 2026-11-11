//! `setTimeout`/`setInterval`: the pending [`Timer`]s of a mod and how the
//! mod thread's loop fires them (earliest first, in set order on ties, with
//! the scope of the callback that set them).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::Manager;
use super::core::Core;
use crate::host::Scope;

/// Orders timers that fall due at the same instant across mods (set order).
static TIMER_SEQ: AtomicU64 = AtomicU64::new(0);

/// A pending `setTimeout` / `setInterval`. It fires with the scope of the
/// callback that created it, so a timer cannot widen what the mod may touch.
#[derive(Debug, Clone)]
pub(super) struct Timer {
    due: Instant,
    seq: u64,
    interval: Option<Duration>,
    scope: Scope,
}

impl Core {
    /// Adds a pending timer firing in `delay` (every `delay` when
    /// `repeat`) with the current call's scope. The host call has checked
    /// the delay and the timer quota. Returns the id: the next free one
    /// from `next_timer`, wrapping past `u32::MAX` to 1.
    pub(super) fn add_timer(&mut self, delay: Duration, repeat: bool) -> u32 {
        let mut id = self.next_timer.max(1);
        while self.timers.contains_key(&id) {
            id = id.checked_add(1).unwrap_or(1);
        }
        self.next_timer = id.checked_add(1).unwrap_or(1);
        let timer = Timer {
            due: Instant::now() + delay,
            seq: TIMER_SEQ.fetch_add(1, Ordering::Relaxed),
            interval: repeat.then_some(delay),
            scope: self.scope.clone(),
        };
        self.timers.insert(id, timer);
        id
    }
}

impl Manager {
    /// The earliest pending timer of a running mod.
    pub(super) fn next_due(&self) -> Option<Instant> {
        self.slots
            .values()
            .filter(|s| s.rt.is_some())
            .filter_map(|s| s.core.borrow().timers.values().map(|t| t.due).min())
            .min()
    }

    /// Run every timer that is due now, earliest first (ties in set order).
    /// Timers that fall due while these run wait for the next turn of the
    /// loop, so queued input is never starved by a burst of timers.
    pub(super) fn fire_timers(&mut self) {
        let now = Instant::now();
        let mut due: Vec<(Instant, u64, String, u32)> = Vec::new();
        for (name, s) in self.slots.iter().filter(|(_, s)| s.rt.is_some()) {
            for (id, t) in s.core.borrow().timers.iter().filter(|(_, t)| t.due <= now) {
                due.push((t.due, t.seq, name.clone(), *id));
            }
        }
        due.sort();
        for (_, _, name, id) in due {
            // An earlier callback may have cleared it, or the mod stopped.
            let scope = {
                let Some(slot) = self.slots.get(&name).filter(|s| s.rt.is_some()) else {
                    continue;
                };
                let mut c = slot.core.borrow_mut();
                let Some(t) = c.timers.get_mut(&id) else {
                    continue;
                };
                let scope = t.scope.clone();
                match t.interval {
                    // From now, not from the old due time: no catch-up bursts.
                    Some(every) => t.due = Instant::now() + every,
                    None => {
                        c.timers.remove(&id);
                    }
                }
                scope
            };
            let _ = self.call(&name, "timer", &id.to_string(), "{}", &scope);
        }
    }
}
