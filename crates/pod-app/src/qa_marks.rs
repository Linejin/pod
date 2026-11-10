//! Element bounds for native-QA tests that work in every build profile.
//!
//! GPUI records `debug_selector` bounds only with `debug_assertions` (its own
//! cfg), so `cargo test --release` saw no bounds and the native-QA tests that
//! look elements up failed. `qa_id` replaces it: in test builds it appends an
//! absolutely positioned, paint-free `canvas` that fills its parent (Taffy
//! places absolute children relative to the parent itself) and records the
//! parent's bounds on prepaint; outside tests it is a no-op and compiles to
//! nothing. The harness clears the record before the full-refresh frame it
//! reads (`native_qa::Harness::settle`), so a missing element has no entry.

use gpui_kit::*;

pub trait QaId: ParentElement + Sized {
    /// Name this element for `native_qa::Harness::bounds`.
    #[cfg(test)]
    fn qa_id(self, id: impl FnOnce() -> String) -> Self {
        let id = id();
        self.child(
            canvas(move |bounds, _, _| record(id, bounds), |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
        )
    }

    #[cfg(not(test))]
    #[inline(always)]
    fn qa_id(self, _id: impl FnOnce() -> String) -> Self {
        self
    }
}

impl<E: ParentElement> QaId for E {}

#[cfg(test)]
thread_local! {
    static BOUNDS: std::cell::RefCell<std::collections::HashMap<String, Bounds<Pixels>>> = Default::default();
}

#[cfg(test)]
pub fn record(id: String, bounds: Bounds<Pixels>) {
    BOUNDS.with(|b| b.borrow_mut().insert(id, bounds));
}

/// Forget every recorded element (call before a full-refresh frame).
#[cfg(test)]
pub fn clear() {
    BOUNDS.with(|b| b.borrow_mut().clear());
}

/// Bounds of `id` as painted since the last [`clear`].
#[cfg(test)]
pub fn bounds(id: &str) -> Option<Bounds<Pixels>> {
    BOUNDS.with(|b| b.borrow().get(id).copied())
}
