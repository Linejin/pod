//! Start a background process (podd, pod-holder) fully detached from the
//! caller: no console, no window, new process group, outside the caller's Job
//! Object when allowed, and — the reason this is not `std::process::Command`
//! — **no inherited handles** except a `NUL` handle used as its stdin, stdout
//! and stderr.
//!
//! `Command::spawn` passes `bInheritHandles = TRUE`, so the child inherits
//! every inheritable handle of the caller, not just the three standard ones.
//! A `pod ls | more` that auto-started podd therefore handed podd the write
//! end of the caller's stdout pipe, and the reader never saw EOF until podd
//! exited (docs/09 §5-10). Here the inheritable set is limited with
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` to one freshly opened `NUL` handle.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::process::Command;

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

const ERROR_ACCESS_DENIED: i32 = 5;

/// A process started by [`spawn_detached`]. Dropping it only closes our
/// handle; the process keeps running.
#[derive(Debug)]
pub struct DetachedChild {
    handle: HANDLE,
    pid: u32,
}

// SAFETY: a process handle may be used from any thread.
unsafe impl Send for DetachedChild {}
unsafe impl Sync for DetachedChild {}

impl DetachedChild {
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// `Some(exit code)` once the process has ended.
    pub fn try_wait(&self) -> io::Result<Option<u32>> {
        // SAFETY: valid process handle owned by self.
        unsafe {
            if WaitForSingleObject(self.handle, 0) != WAIT_OBJECT_0 {
                return Ok(None);
            }
            let mut code = 0u32;
            if GetExitCodeProcess(self.handle, &mut code) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(code))
        }
    }

    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: valid process handle owned by self.
        if unsafe { TerminateProcess(self.handle, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for DetachedChild {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { CloseHandle(self.handle) };
    }
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Append `arg` quoted for `CommandLineToArgvW` / the MSVC CRT.
fn append_arg(cmd: &mut Vec<u16>, arg: &OsStr) {
    if !cmd.is_empty() {
        cmd.push(b' ' as u16);
    }
    let a: Vec<u16> = arg.encode_wide().collect();
    let needs_quotes = a.is_empty()
        || a.iter()
            .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
    if !needs_quotes {
        cmd.extend_from_slice(&a);
        return;
    }
    cmd.push(b'"' as u16);
    let mut backslashes = 0usize;
    for &c in &a {
        if c == b'\\' as u16 {
            backslashes += 1;
        } else {
            if c == b'"' as u16 {
                // Escape the preceding backslashes and the quote itself.
                cmd.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
            }
            backslashes = 0;
        }
        cmd.push(c);
    }
    // Backslashes before the closing quote must be doubled.
    cmd.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
    cmd.push(b'"' as u16);
}

/// The command line `CreateProcessW` gets for `program` + `args`.
pub fn command_line(program: &OsStr, args: &[&OsStr]) -> Vec<u16> {
    let mut cmd = Vec::new();
    // The program name is parsed differently (no escapes); quote it whole.
    let p: Vec<u16> = program.encode_wide().collect();
    cmd.push(b'"' as u16);
    cmd.extend_from_slice(&p);
    cmd.push(b'"' as u16);
    for a in args {
        append_arg(&mut cmd, a);
    }
    cmd.push(0);
    cmd
}

/// Environment block: ours plus the command's overrides (sorted, as Windows wants).
fn env_block(cmd: &Command) -> Option<Vec<u16>> {
    let mut envs = cmd.get_envs().peekable();
    envs.peek()?;
    let mut map: std::collections::BTreeMap<OsString, (OsString, OsString)> = std::env::vars_os()
        .map(|(k, v)| (k.to_ascii_uppercase(), (k, v)))
        .collect();
    for (k, v) in envs {
        match v {
            Some(v) => {
                map.insert(k.to_ascii_uppercase(), (k.to_os_string(), v.to_os_string()));
            }
            None => {
                map.remove(&k.to_ascii_uppercase());
            }
        }
    }
    let mut block = Vec::new();
    for (k, v) in map.values() {
        block.extend(k.encode_wide());
        block.push(b'=' as u16);
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    Some(block)
}

/// An inheritable handle to `NUL` (read + write).
fn open_nul() -> io::Result<OwnedHandle> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let name = wide(OsStr::new("NUL"));
    // SAFETY: valid, NUL-terminated name and attributes for the call.
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &sa,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(h))
}

/// Start `cmd` (program, args, current dir, env overrides; its stdio settings
/// are ignored) detached and with nothing inherited but `NUL` as stdio.
pub fn spawn_detached(cmd: &Command) -> io::Result<DetachedChild> {
    let program = cmd.get_program();
    let args: Vec<&OsStr> = cmd.get_args().collect();
    let cwd = cmd.get_current_dir().map(|d| wide(d.as_os_str()));
    let env = env_block(cmd);
    let nul = open_nul()?;

    // Attribute list with one attribute: the handle list.
    let mut size = 0usize;
    // SAFETY: size query; fails with ERROR_INSUFFICIENT_BUFFER by design.
    unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
    let mut attr_buf = vec![0u8; size.max(64)];
    let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: buffer of the requested size.
    if unsafe { InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) } == 0 {
        return Err(io::Error::last_os_error());
    }
    struct AttrGuard(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttrGuard {
        fn drop(&mut self) {
            // SAFETY: initialized list.
            unsafe { DeleteProcThreadAttributeList(self.0) };
        }
    }
    let _guard = AttrGuard(attrs);
    let handles: [HANDLE; 1] = [nul.0];
    // SAFETY: `handles` outlives CreateProcessW below.
    if unsafe {
        UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr() as *const _,
            std::mem::size_of_val(&handles),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: zeroed is a valid STARTUPINFOEXW.
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = nul.0;
    si.StartupInfo.hStdOutput = nul.0;
    si.StartupInfo.hStdError = nul.0;
    si.lpAttributeList = attrs;

    let base = DETACHED_PROCESS
        | CREATE_NEW_PROCESS_GROUP
        | CREATE_NO_WINDOW
        | EXTENDED_STARTUPINFO_PRESENT
        | CREATE_UNICODE_ENVIRONMENT;
    let try_spawn = |flags: u32| -> io::Result<DetachedChild> {
        let mut cmdline = command_line(program, &args);
        // SAFETY: zeroed is a valid PROCESS_INFORMATION.
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: every pointer is valid for the duration of the call;
        // `cmdline` is mutable as CreateProcessW requires.
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // only the handle list is inherited
                flags,
                env.as_ref()
                    .map_or(std::ptr::null(), |e| e.as_ptr() as *const _),
                cwd.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                &si.StartupInfo,
                &mut pi,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: we own the thread handle and do not need it.
        unsafe { CloseHandle(pi.hThread) };
        Ok(DetachedChild {
            handle: pi.hProcess,
            pid: pi.dwProcessId,
        })
    };
    match try_spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(c) => Ok(c),
        // Our job forbids breakaway: start inside it instead.
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => try_spawn(base),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cl(program: &str, args: &[&str]) -> String {
        let a: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let w = command_line(OsStr::new(program), &a);
        String::from_utf16(&w[..w.len() - 1]).unwrap()
    }

    #[test]
    fn quoting_follows_the_crt_rules() {
        assert_eq!(
            cl(
                r"C:\Program Files\pod\podd.exe",
                &["--pipe", r"\\.\pipe\podd-x"]
            ),
            r#""C:\Program Files\pod\podd.exe" --pipe \\.\pipe\podd-x"#
        );
        assert_eq!(cl("x", &["a b", ""]), r#""x" "a b" """#);
        assert_eq!(cl("x", &[r#"say "hi""#]), r#""x" "say \"hi\"""#);
        assert_eq!(
            cl("x", &[r"C:\dir with space\"]),
            r#""x" "C:\dir with space\\""#
        );
        assert_eq!(cl("x", &[r"a\\b"]), r#""x" a\\b"#);
    }

    /// The child gets NUL as stdio and none of our inheritable handles: an
    /// inheritable pipe we hold must reach EOF once we close our write end,
    /// even while the child still runs.
    #[test]
    fn child_does_not_inherit_our_pipe() {
        use std::io::Read;
        use std::os::windows::io::{FromRawHandle, OwnedHandle as StdOwned};
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows_sys::Win32::System::Pipes::CreatePipe;
        let (mut r, mut w): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: out-pointers are valid.
        assert_ne!(
            unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) },
            0
        );
        // SAFETY: make the write end inheritable, like a redirected stdout.
        unsafe { SetHandleInformation(w, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
        // SAFETY: we own both handles.
        let (r, w) = unsafe {
            (
                std::fs::File::from(StdOwned::from_raw_handle(r as _)),
                StdOwned::from_raw_handle(w as _),
            )
        };
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "ping -n 6 127.0.0.1 >nul"]);
        let child = spawn_detached(&cmd).expect("spawn");
        assert!(child.try_wait().unwrap().is_none());
        drop(w);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = r;
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            let _ = tx.send(());
        });
        let eof = rx.recv_timeout(std::time::Duration::from_secs(3)).is_ok();
        let still_running = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        assert!(eof, "the pipe stayed open: the child inherited it");
        assert!(still_running, "the child should still be running");
    }
}
