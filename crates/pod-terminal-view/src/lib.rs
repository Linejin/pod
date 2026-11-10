//! Reusable GPUI terminal view for Pod.
//!
//! The view does **not** own a PTY. It owns a VT model that is fed raw program
//! output with [`TerminalView::feed`], and sends everything the user does
//! (keys, IME commits, paste, mouse reports, query replies, resizes) to an
//! [`InputSink`]. pod-spike plugs a local ConPTY in; pod-app will plug a podd
//! attach connection in.
//!
//! ```ignore
//! let view = cx.new(|cx| TerminalView::new(Box::new(my_sink), TerminalConfig::default(), window, cx));
//! // bytes from the PTY / daemon:
//! view.update(cx, |v, cx| v.feed(&bytes, cx));
//! // embed (cached: only re-renders when this terminal changed):
//! div().child(view.clone().cached(StyleRefinement::default().size_full()))
//! ```

pub mod boxdraw;
pub mod element;
pub mod keys;
pub mod links;
pub mod mouse;
pub mod palette;
pub mod perf;
pub mod selection;
pub mod sink;
pub mod view;
pub mod vt;

pub use links::{LinkTarget, TerminalLink};
pub use sink::{FnSink, InputSink, NullSink, TermSize};
pub use view::{TerminalConfig, TerminalEvent, TerminalView};
pub use vt::{GridSize, Modes, MouseTracking};

/// `POD_TERM_DEBUG=1` prints input-handling traces to stdout.
#[doc(hidden)]
pub fn debug_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("POD_TERM_DEBUG").is_some())
}

#[doc(hidden)]
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => {
        if $crate::debug_enabled() {
            println!("[pod-term] {}", format!($($t)*));
        }
    };
}

/// Default scrollback length in lines.
pub const DEFAULT_SCROLLBACK: usize = 5000;

pub use palette::TerminalPalette;
