//! Where the view sends what the user types.
//!
//! The view never owns a PTY. Today the sink is a local ConPTY (pod-spike);
//! in pod-app it is a podd attach connection that forwards bytes to the
//! daemon-held PTY.

/// Terminal size as sent to the program (and to ConPTY / podd).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
    /// Cell size in device-independent pixels (for `TIOCGWINSZ` pixel fields / CSI 14 t).
    pub cell_width: u16,
    pub cell_height: u16,
}

/// Abstract output side of a terminal view.
///
/// Implementations must not block the UI thread for long: queue the bytes
/// (channel, socket buffer) and return.
pub trait InputSink: 'static {
    /// Bytes for the program: encoded keys, pasted text, mouse reports and
    /// replies to terminal queries (DA, DSR, OSC 10/11 …).
    fn write(&self, bytes: &[u8]);

    /// The grid changed size. Called after the view's own VT was resized.
    fn resize(&self, size: TermSize);
}

/// A sink that discards everything (tests, read-only replays).
pub struct NullSink;

impl InputSink for NullSink {
    fn write(&self, _: &[u8]) {}
    fn resize(&self, _: TermSize) {}
}

/// Adapter so a pair of closures can act as a sink.
pub struct FnSink<W, R>
where
    W: Fn(&[u8]) + 'static,
    R: Fn(TermSize) + 'static,
{
    pub write: W,
    pub resize: R,
}

impl<W, R> InputSink for FnSink<W, R>
where
    W: Fn(&[u8]) + 'static,
    R: Fn(TermSize) + 'static,
{
    fn write(&self, bytes: &[u8]) {
        (self.write)(bytes)
    }
    fn resize(&self, size: TermSize) {
        (self.resize)(size)
    }
}
