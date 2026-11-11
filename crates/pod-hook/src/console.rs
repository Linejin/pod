//! Console for the CLI paths (`install`, help, version). pod-hook is a
//! GUI-subsystem exe on Windows, so it never gets a console of its own; the
//! hook path never calls this.

/// GUI-subsystem exe started from a console without redirection: write to the
/// parent's console (best effort; the shell may print its prompt first).
#[cfg(windows)]
pub(crate) fn use_parent_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };
    let missing = |id: STD_HANDLE| {
        // SAFETY: plain query of this process's standard handle.
        let h = unsafe { GetStdHandle(id) };
        h.is_null() || h == INVALID_HANDLE_VALUE
    };
    let (out, err) = (missing(STD_OUTPUT_HANDLE), missing(STD_ERROR_HANDLE));
    // SAFETY: no pointers; fails harmlessly without a parent console.
    if !(out || err) || unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    for (id, want) in [(STD_OUTPUT_HANDLE, out), (STD_ERROR_HANDLE, err)] {
        if want && let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            // SAFETY: the handle is owned by us and stays open for the process lifetime.
            unsafe { SetStdHandle(id, f.into_raw_handle()) };
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn use_parent_console() {}
