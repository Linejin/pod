//! `pod attach`: an interactive client in the current Windows console.
//!
//! Three connections to the daemon: the attach stream (snapshot + raw output,
//! read on its own thread), and a control connection shared by the stdin
//! thread (`terminal.input`) and the size watcher (`terminal.resize`).
//! Synchronous pipe handles serialize I/O, so reading and writing must not
//! share one connection.
//!
//! The console is put in raw VT mode: input arrives as VT bytes
//! (`ENABLE_VIRTUAL_TERMINAL_INPUT`, no line/echo/processed input) and output
//! is written as-is (`ENABLE_VIRTUAL_TERMINAL_PROCESSING`). Ctrl+] detaches.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use podd::client::{AttachEvent, Client};
use podd::protocol::*;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{
    CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_EXTENDED_FLAGS,
    ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    GetConsoleCP, GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo, GetStdHandle,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCP, SetConsoleMode, SetConsoleOutputCP,
};

/// Ctrl+] — the detach key.
const DETACH: u8 = 0x1d;

/// Undo modes a program may have left on, after we stop forwarding its output.
const RESET: &[u8] = b"\x1b[0m\x1b[?25h\x1b[?1049l\x1b[?2004l\x1b[?1l\x1b>\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?7h";

pub struct AttachOptions {
    pub terminal: String,
    pub scrollback: usize,
    /// Resize the terminal to this console (and follow console resizes).
    pub take_size: bool,
}

enum Done {
    Detached,
    Exited(ExitedEvent),
    StreamClosed,
    Error(String),
}

struct Console {
    stdin: HANDLE,
    stdout: HANDLE,
    in_mode: CONSOLE_MODE,
    out_mode: CONSOLE_MODE,
    in_cp: u32,
    out_cp: u32,
}

impl Console {
    fn raw() -> anyhow::Result<Self> {
        unsafe {
            let stdin = GetStdHandle(STD_INPUT_HANDLE);
            let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
            if stdin == INVALID_HANDLE_VALUE || stdout == INVALID_HANDLE_VALUE {
                bail!("no console handles");
            }
            let (mut in_mode, mut out_mode) = (0, 0);
            if GetConsoleMode(stdin, &mut in_mode) == 0
                || GetConsoleMode(stdout, &mut out_mode) == 0
            {
                bail!(
                    "pod attach needs an interactive console (stdin/stdout must not be redirected)"
                );
            }
            let c = Self {
                stdin,
                stdout,
                in_mode,
                out_mode,
                in_cp: GetConsoleCP(),
                out_cp: GetConsoleOutputCP(),
            };
            SetConsoleCP(65001);
            SetConsoleOutputCP(65001);
            SetConsoleMode(stdin, ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS);
            SetConsoleMode(
                stdout,
                ENABLE_PROCESSED_OUTPUT
                    | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                    | DISABLE_NEWLINE_AUTO_RETURN,
            );
            Ok(c)
        }
    }

    fn size(&self) -> Option<(u16, u16)> {
        size_of_console(self.stdout)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        unsafe {
            SetConsoleMode(self.stdin, self.in_mode);
            SetConsoleMode(self.stdout, self.out_mode);
            SetConsoleCP(self.in_cp);
            SetConsoleOutputCP(self.out_cp);
        }
    }
}

fn size_of_console(out: HANDLE) -> Option<(u16, u16)> {
    unsafe {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(out, &mut info) == 0 {
            return None;
        }
        let w = info.srWindow;
        Some(((w.Right - w.Left + 1) as u16, (w.Bottom - w.Top + 1) as u16))
    }
}

/// Raw write to the console (no UTF-8 validation: chunks may split characters).
fn write_out(out: HANDLE, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let mut n = 0u32;
        let ok = unsafe {
            WriteFile(
                out,
                bytes.as_ptr(),
                bytes.len().min(1 << 20) as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || n == 0 {
            return;
        }
        bytes = &bytes[n as usize..];
    }
}

struct SendHandle(HANDLE);
// SAFETY: console handles are process-global and usable from any thread.
unsafe impl Send for SendHandle {}

pub fn run(pipe: &str, opts: AttachOptions) -> anyhow::Result<i32> {
    let target = TerminalRef::parse(&opts.terminal);
    let console = Console::raw()?;
    let size = if opts.take_size { console.size() } else { None };

    let stream_client = Client::connect_or_start(pipe, None)
        .with_context(|| format!("cannot reach podd on {pipe}"))?;
    let control = Arc::new(Mutex::new(Client::connect(pipe)?));
    let params = AttachParams {
        target: target.clone(),
        scrollback: Some(opts.scrollback),
        cols: size.map(|s| s.0),
        rows: size.map(|s| s.1),
    };
    let (result, snapshot, mut stream) = stream_client.attach(&params)?;
    let id = result.terminal.id.clone();
    let target = TerminalRef::id(&id);
    write_out(console.stdout, &snapshot);

    let (tx, rx) = mpsc::channel::<Done>();

    // Output: stream events -> console, checking that offsets are contiguous.
    {
        let tx = tx.clone();
        let out = SendHandle(console.stdout);
        let mut expect = result.seq;
        let pipe = pipe.to_string();
        let target = target.clone();
        let scrollback = opts.scrollback;
        std::thread::spawn(move || {
            let out = out;
            loop {
                match stream.next_event() {
                    // Too slow (the daemon's per-client queue is bounded):
                    // re-attach from a fresh snapshot and carry on.
                    Ok(Some(AttachEvent::Lagged(_))) => {
                        let again = Client::connect(&pipe).and_then(|c| {
                            c.attach(&AttachParams {
                                target: target.clone(),
                                scrollback: Some(scrollback),
                                cols: None,
                                rows: None,
                            })
                        });
                        match again {
                            Ok((r, snapshot, s)) => {
                                write_out(out.0, RESET);
                                write_out(out.0, &snapshot);
                                expect = r.seq;
                                stream = s;
                            }
                            Err(e) => {
                                let _ = tx.send(Done::Error(format!("re-attach failed: {e}")));
                                return;
                            }
                        }
                    }
                    Ok(Some(AttachEvent::Output { seq, data })) => {
                        if seq != expect {
                            let _ = tx.send(Done::Error(format!(
                                "output gap: expected offset {expect}, got {seq}"
                            )));
                            return;
                        }
                        expect = seq + data.len() as u64;
                        write_out(out.0, &data);
                    }
                    Ok(Some(AttachEvent::Resized { .. })) => {}
                    Ok(Some(AttachEvent::Exited(e))) => {
                        let _ = tx.send(Done::Exited(e));
                        return;
                    }
                    Ok(None) => {
                        let _ = tx.send(Done::StreamClosed);
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Done::Error(e.to_string()));
                        return;
                    }
                }
            }
        });
    }

    // Input: console bytes -> terminal.input, until Ctrl+].
    {
        let tx = tx.clone();
        let control = control.clone();
        let target = target.clone();
        let input = SendHandle(console.stdin);
        std::thread::spawn(move || {
            let input = input;
            let mut buf = [0u8; 4096];
            loop {
                let mut n = 0u32;
                let ok = unsafe {
                    ReadFile(
                        input.0,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut n,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    let _ = tx.send(Done::Error("console input closed".into()));
                    return;
                }
                let bytes = &buf[..n as usize];
                let (send, detach) = match bytes.iter().position(|&b| b == DETACH) {
                    Some(i) => (&bytes[..i], true),
                    None => (bytes, false),
                };
                if !send.is_empty() {
                    let p = InputParams {
                        target: target.clone(),
                        data: b64_encode(send),
                    };
                    if let Err(e) = control.lock().unwrap().call::<TerminalInput>(&p) {
                        let _ = tx.send(Done::Error(e.to_string()));
                        return;
                    }
                }
                if detach {
                    let _ = tx.send(Done::Detached);
                    return;
                }
            }
        });
    }

    // Size: follow console resizes (most recent resize wins across clients).
    if opts.take_size {
        let out = SendHandle(console.stdout);
        let control = control.clone();
        let target = target.clone();
        let mut last = size;
        std::thread::spawn(move || {
            let out = out;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let now = size_of_console(out.0);
                if let Some((cols, rows)) = now
                    && now != last
                {
                    last = now;
                    let _ = control
                        .lock()
                        .unwrap()
                        .call::<TerminalResize>(&ResizeParams {
                            target: target.clone(),
                            cols,
                            rows,
                        });
                }
            }
        });
    }

    let done = rx.recv().unwrap_or(Done::StreamClosed);
    write_out(console.stdout, RESET);
    let (msg, code) = match done {
        Done::Detached => (format!("[pod: detached from {id}]"), 0),
        Done::Exited(e) => (
            format!(
                "[pod: {id} exited{}]",
                e.exit_code
                    .map(|c| format!(" with code {c}"))
                    .unwrap_or_default()
            ),
            0,
        ),
        Done::StreamClosed => (format!("[pod: connection to podd closed ({id})]"), 2),
        Done::Error(e) => (format!("[pod: {e}]"), 2),
    };
    write_out(console.stdout, format!("\r\n{msg}\r\n").as_bytes());
    drop(console);
    // The stdin thread may still be blocked in ReadFile; exit without joining it.
    std::process::exit(code);
}
