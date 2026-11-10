//! ConPTY child process inside a Job Object (used by the holder).
//!
//! We create the pseudo console ourselves instead of using alacritty's event
//! loop because the holder needs (a) the raw output bytes, to stream them and
//! to count them, and (b) the child placed in a Job Object *before* it runs,
//! so `kill` takes down the whole process tree (`CREATE_SUSPENDED` ->
//! `AssignProcessToJobObject` -> `ResumeThread`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

use crate::env::get_var;

type CreateFn = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> i32;
type ResizeFn = unsafe extern "system" fn(HPCON, COORD) -> i32;
type CloseFn = unsafe extern "system" fn(HPCON);

/// Which ConPTY implementation creates the pseudo consoles.
///
/// The Windows inbox ConPTY (kernel32 + the system conhost) swallows some
/// input sequences, e.g. the OSC 7501 query reply Claude Code waits for
/// (docs/04 §1.4). Microsoft ships a newer ConPTY as a NuGet package
/// (`Microsoft.Windows.Console.ConPTY`, MIT): `conpty.dll` + `OpenConsole.exe`.
/// When `conpty.dll` sits next to the holder executable (with
/// `x64\OpenConsole.exe`, the package layout) we load and use it.
pub struct ConptyApi {
    create: CreateFn,
    resize: ResizeFn,
    close: CloseFn,
    /// `"bundled:<path>"` or `"inbox"`.
    pub kind: String,
}

/// Search order: `POD_CONPTY=inbox` forces the inbox ConPTY; `POD_CONPTY=<dir>`
/// names a directory with `conpty.dll`; otherwise the executable's directory.
pub fn conpty_api() -> &'static ConptyApi {
    static API: std::sync::OnceLock<ConptyApi> = std::sync::OnceLock::new();
    API.get_or_init(|| {
        let inbox = ConptyApi {
            create: CreatePseudoConsole,
            resize: ResizePseudoConsole,
            close: ClosePseudoConsole,
            kind: "inbox".into(),
        };
        let dir = match std::env::var("POD_CONPTY") {
            Ok(v) if v.eq_ignore_ascii_case("inbox") => return inbox,
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => match std::env::current_exe() {
                Ok(exe) => exe.parent().map(Path::to_path_buf).unwrap_or_default(),
                Err(_) => return inbox,
            },
        };
        let dll = dir.join("conpty.dll");
        if !dll.is_file() {
            return inbox;
        }
        unsafe {
            let module = LoadLibraryExW(
                wide(dll.as_os_str()).as_ptr(),
                std::ptr::null_mut(),
                LOAD_WITH_ALTERED_SEARCH_PATH,
            );
            if module.is_null() {
                return inbox;
            }
            let create = GetProcAddress(module, c"ConptyCreatePseudoConsole".as_ptr().cast());
            let resize = GetProcAddress(module, c"ConptyResizePseudoConsole".as_ptr().cast());
            let close = GetProcAddress(module, c"ConptyClosePseudoConsole".as_ptr().cast());
            match (create, resize, close) {
                (Some(c), Some(r), Some(x)) => ConptyApi {
                    create: std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateFn>(
                        c,
                    ),
                    resize: std::mem::transmute::<unsafe extern "system" fn() -> isize, ResizeFn>(
                        r,
                    ),
                    close: std::mem::transmute::<unsafe extern "system" fn() -> isize, CloseFn>(x),
                    kind: format!("bundled:{}", dll.display()),
                },
                _ => inbox,
            }
        }
    })
}

struct Handle(HANDLE);

// SAFETY: kernel object handles may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

pub struct Pty {
    hpc: HPCON,
    closed: AtomicBool,
    input: Mutex<File>,
    process: Handle,
    job: Handle,
    pid: u32,
}

// SAFETY: HPCON is an opaque handle; ConPTY calls are thread-safe and
// ClosePseudoConsole is guarded by `closed`.
unsafe impl Send for Pty {}
unsafe impl Sync for Pty {}

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Pty {
    /// Start `argv` on a new pseudo console. Returns the PTY and the reader for
    /// its output. `env` is the complete environment of the child.
    pub fn spawn(
        argv: &[String],
        cwd: &Path,
        env: &BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> io::Result<(Self, File)> {
        let cmdline = command_line(argv, env)?;
        unsafe {
            let (mut in_read, mut in_write, mut out_read, mut out_write) = (
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            check(CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0))?;
            let (in_read, in_write) = (Handle(in_read), Handle(in_write));
            check(CreatePipe(
                &mut out_read,
                &mut out_write,
                std::ptr::null(),
                0,
            ))?;
            let (out_read, out_write) = (Handle(out_read), Handle(out_write));

            let mut hpc: HPCON = 0;
            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            let api = conpty_api();
            let hr = (api.create)(size, in_read.0, out_write.0, 0, &mut hpc);
            if hr < 0 {
                return Err(io::Error::from_raw_os_error(hr));
            }
            // The pseudo console holds its own duplicates of these ends.
            drop(in_read);
            drop(out_write);

            let close_on_err = |e: io::Error| {
                (api.close)(hpc);
                e
            };

            let mut attr_size = 0usize;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
            let mut attr_buf = vec![0u8; attr_size];
            let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            check(InitializeProcThreadAttributeList(
                attrs,
                1,
                0,
                &mut attr_size,
            ))
            .map_err(close_on_err)?;
            let attr_result = check(UpdateProcThreadAttribute(
                attrs,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ));
            if let Err(e) = attr_result {
                DeleteProcThreadAttributeList(attrs);
                return Err(close_on_err(e));
            }

            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // Null std handles: the child must use the pseudo console, never
            // whatever the holder's own std handles are.
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.lpAttributeList = attrs;

            let mut cmd = wide(&cmdline);
            let cwd_w = wide(cwd.as_os_str());
            let env_block = env_block(env);
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let created = check(CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                env_block.as_ptr() as *const c_void,
                cwd_w.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            ));
            DeleteProcThreadAttributeList(attrs);
            if let Err(e) = created {
                return Err(close_on_err(io::Error::new(
                    e.kind(),
                    format!("cannot start {cmdline:?}: {e}"),
                )));
            }
            let process = Handle(pi.hProcess);
            let thread = Handle(pi.hThread);

            // Job Object: the whole tree dies on kill, or if the holder dies.
            let job = Handle(CreateJobObjectW(std::ptr::null(), std::ptr::null()));
            if job.0.is_null() {
                return Err(close_on_err(io::Error::last_os_error()));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            check(SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ))
            .map_err(close_on_err)?;
            check(AssignProcessToJobObject(job.0, process.0)).map_err(close_on_err)?;
            ResumeThread(thread.0);
            drop(thread);

            let input = File::from_raw_handle(in_write.0 as _);
            std::mem::forget(in_write);
            let output = File::from_raw_handle(out_read.0 as _);
            std::mem::forget(out_read);

            Ok((
                Self {
                    hpc,
                    closed: AtomicBool::new(false),
                    input: Mutex::new(input),
                    process,
                    job,
                    pid: pi.dwProcessId,
                },
                output,
            ))
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pseudo console closed",
            ));
        }
        let mut input = self.input.lock().unwrap();
        input.write_all(bytes)?;
        input.flush()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let hr = unsafe {
            (conpty_api().resize)(
                self.hpc,
                COORD {
                    X: cols as i16,
                    Y: rows as i16,
                },
            )
        };
        if hr < 0 {
            Err(io::Error::from_raw_os_error(hr))
        } else {
            Ok(())
        }
    }

    /// Terminate every process in the job (the child and all descendants).
    pub fn kill_tree(&self, code: u32) {
        unsafe { TerminateJobObject(self.job.0, code) };
    }

    /// Block until the root process exits; returns its exit code.
    pub fn wait(&self) -> Option<i32> {
        unsafe {
            if WaitForSingleObject(self.process.0, INFINITE) != WAIT_OBJECT_0 {
                return None;
            }
            let mut code = 0u32;
            (GetExitCodeProcess(self.process.0, &mut code) != 0).then_some(code as i32)
        }
    }

    /// Close the pseudo console: flushes the last output, ends console clients
    /// still attached and makes the output reader see end-of-file.
    pub fn close_console(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            unsafe { (conpty_api().close)(self.hpc) };
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close_console();
    }
}

/// UTF-16 environment block, sorted case-insensitively as Windows expects.
fn env_block(env: &BTreeMap<String, String>) -> Vec<u16> {
    let mut vars: Vec<(&String, &String)> = env.iter().collect();
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(OsStr::new(&format!("{k}={v}")).encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// Quote one argument for `CommandLineToArgvW` / the MSVC runtime.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// Resolve `argv[0]` through the child's `PATH`/`PATHEXT` (CreateProcess alone
/// only finds `.exe`), and run `.cmd`/`.bat` shims such as npm's `claude.cmd`
/// through `cmd.exe /d /s /c`.
pub fn command_line(argv: &[String], env: &BTreeMap<String, String>) -> io::Result<String> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "argv is empty"))?;
    let resolved = resolve_program(program, env).unwrap_or_else(|| PathBuf::from(program));
    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut parts = vec![quote_arg(&resolved.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote_arg(a)));
    let line = parts.join(" ");
    if ext == "cmd" || ext == "bat" {
        Ok(format!("cmd.exe /d /s /c \"{line}\""))
    } else {
        Ok(line)
    }
}

/// True when `argv` would run through `cmd.exe` (its program resolves to a
/// `.cmd`/`.bat` shim), where `%`, `"` and line breaks in arguments are not safe.
pub fn runs_through_cmd(argv: &[String], env: &BTreeMap<String, String>) -> bool {
    command_line(argv, env).is_ok_and(|l| l.starts_with("cmd.exe /d /s /c \""))
}

fn resolve_program(program: &str, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 || p.is_absolute() {
        return None; // explicit path: CreateProcess handles it
    }
    let pathext = get_var(env, "PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD");
    let exts: Vec<String> = pathext
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    let has_ext = p.extension().is_some_and(|e| {
        exts.iter()
            .any(|x| x.trim_start_matches('.') == e.to_ascii_lowercase())
    });
    let path = get_var(env, "PATH")?;
    for dir in std::env::split_paths(path) {
        if has_ext {
            let c = dir.join(program);
            if c.is_file() {
                return Some(c);
            }
            continue;
        }
        for ext in &exts {
            let c = dir.join(format!("{program}{ext}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_matches_msvc_rules() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg("two words"), "\"two words\"");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[test]
    fn cmd_shims_go_through_cmd_exe() {
        let dir = std::env::temp_dir().join(format!("podd-pty-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fakeagent.cmd"), "@echo off\r\n").unwrap();
        let mut env = BTreeMap::new();
        env.insert("Path".to_string(), dir.display().to_string());
        env.insert("PATHEXT".to_string(), ".COM;.EXE;.BAT;.CMD".to_string());
        let line =
            command_line(&["fakeagent".into(), "--flag".into(), "a b".into()], &env).unwrap();
        assert!(line.starts_with("cmd.exe /d /s /c \""), "{line}");
        assert!(line.contains("fakeagent.cmd"), "{line}");
        assert!(line.ends_with(" --flag \"a b\"\""), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
