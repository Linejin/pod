//! Duplex byte transports for the NDJSON JSON-RPC client (docs/17 R1).
//!
//! The protocol is one UTF-8 JSON value per `\n`-terminated line in each
//! direction. [`Transport`] carries it over any `Read` + `Write` pair: the
//! local named pipe today (a `File` and its clone), a child's stdout/stdin
//! (`pod remote-stdio` behind `ssh`/`wsl.exe`), or in-memory pipes in tests.
//!
//! [`LineReader`] checks the line limit **before** growing its buffer, so a
//! peer that never sends `\n` cannot make the reader allocate more than the
//! limit. A line cut by EOF, a line over the limit and a line that is not
//! UTF-8 are errors, never handed out as data.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};

/// Line limit on the local named pipe. v11 had none; this only guards
/// against a runaway peer (the largest real lines are attach snapshots).
pub const LOCAL_MAX_LINE: usize = 64 * 1024 * 1024;
/// Line limit of the remote stdio transport (docs/17 §2.3): 8 MiB.
pub const REMOTE_MAX_LINE: usize = 8 * 1024 * 1024;

const READ_BUFFER: usize = 64 * 1024;

/// `CreateFile` flags every Pod named-pipe client opens with: the server
/// (podd, a holder, or whoever squats the pipe name) may identify the
/// client but never impersonate it (security-01).
#[cfg(windows)]
pub const SQOS_IDENTIFICATION: u32 = windows_sys::Win32::Storage::FileSystem::SECURITY_SQOS_PRESENT
    | windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;

/// Open a named pipe for reading and writing as a plain `File`, with
/// [`SQOS_IDENTIFICATION`]. Use this instead of a bare `OpenOptions`.
#[cfg(windows)]
pub fn open_pipe(path: &str) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SQOS_IDENTIFICATION)
        .open(path)
}

/// Why a line could not be read.
#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    /// The peer closed the stream in the middle of a line.
    Truncated,
    /// The line is longer than the limit (bytes, without the `\n`). The
    /// reader is unusable afterwards (the rest of the line is unread).
    TooLarge(usize),
    /// The line is not valid UTF-8 (it was consumed; the next line is fine).
    NotUtf8,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "{e}"),
            FrameError::Truncated => f.write_str("the stream ended in the middle of a line"),
            FrameError::TooLarge(n) => write!(f, "a line is longer than the {n}-byte limit"),
            FrameError::NotUtf8 => f.write_str("a line is not valid UTF-8"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<std::io::Error> for FrameError {
    fn from(e: std::io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Reads `\n`-terminated UTF-8 lines with a size limit.
pub struct LineReader {
    inner: BufReader<Box<dyn Read + Send>>,
    max: usize,
    broken: bool,
}

impl LineReader {
    pub fn new(r: impl Read + Send + 'static, max_line: usize) -> Self {
        Self {
            inner: BufReader::with_capacity(READ_BUFFER, Box::new(r)),
            max: max_line,
            broken: false,
        }
    }

    /// Change the line limit (e.g. the remote limit on a local pipe).
    pub fn set_max_line(&mut self, max_line: usize) {
        self.max = max_line;
    }

    pub fn max_line(&self) -> usize {
        self.max
    }

    /// The next line without its `\n`; `Ok(None)` at a clean EOF (between lines).
    pub fn read_line(&mut self) -> Result<Option<String>, FrameError> {
        self.read_line_bytes()?
            .map(|b| String::from_utf8(b).map_err(|_| FrameError::NotUtf8))
            .transpose()
    }

    /// Like [`LineReader::read_line`] without the UTF-8 check.
    pub fn read_line_bytes(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.broken {
            return Err(FrameError::TooLarge(self.max));
        }
        let mut line: Vec<u8> = Vec::new();
        loop {
            let buf = match self.inner.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                // A named pipe whose server end went away.
                Err(e) if e.kind() == ErrorKind::BrokenPipe && line.is_empty() => {
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            };
            if buf.is_empty() {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            let (take, done) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            // Checked before the bytes are copied: the buffer never grows past the limit.
            let content = line.len() + take - usize::from(done);
            if content > self.max {
                self.broken = true;
                return Err(FrameError::TooLarge(self.max));
            }
            line.extend_from_slice(&buf[..take]);
            self.inner.consume(take);
            if done {
                line.pop();
                return Ok(Some(line));
            }
        }
    }
}

/// The writing half: whole lines, flushed.
pub struct LineWriter {
    inner: Box<dyn Write + Send>,
}

impl LineWriter {
    pub fn new(w: impl Write + Send + 'static) -> Self {
        Self { inner: Box::new(w) }
    }

    /// Write `line` (which must not contain `\n`) plus `\n`, then flush.
    pub fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.write_line_bytes(line.as_bytes())
    }

    pub fn write_line_bytes(&mut self, line: &[u8]) -> std::io::Result<()> {
        // One write per line, so a reader never sees half a line from us
        // interleaved with another writer.
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line);
        buf.push(b'\n');
        self.inner.write_all(&buf)?;
        self.inner.flush()
    }
}

/// Both directions of one connection.
pub struct Transport {
    pub reader: LineReader,
    pub writer: LineWriter,
}

impl Transport {
    /// Any `Read` + `Write` pair: a child's stdout + stdin, in-memory pipes…
    pub fn new(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        max_line: usize,
    ) -> Self {
        Self {
            reader: LineReader::new(r, max_line),
            writer: LineWriter::new(w),
        }
    }

    /// A connected handle read and written through a clone. Only for handles
    /// that are never read and written at the same time: Windows serializes
    /// synchronous I/O on one file object, so a blocked read stalls a write.
    /// Named pipes use [`Transport::open_pipe`].
    pub fn from_file(file: std::fs::File) -> std::io::Result<Self> {
        let writer = file.try_clone()?;
        Ok(Self::new(file, writer, LOCAL_MAX_LINE))
    }

    /// Connect to a named pipe for full-duplex use: overlapped I/O, so one
    /// thread can block reading while another writes (`pod remote-stdio`).
    /// Errors keep the OS codes (`NotFound`, `ERROR_PIPE_BUSY` = 231).
    pub fn open_pipe(path: &str) -> std::io::Result<Self> {
        let (r, w) = overlapped::open(path)?;
        Ok(Self::new(r, w, LOCAL_MAX_LINE))
    }

    /// The stdout + stdin of a child started with piped stdio (e.g. `ssh … pod
    /// remote-stdio`), with the remote line limit. `None` if either is not piped.
    pub fn from_child(child: &mut std::process::Child) -> Option<Self> {
        let out = child.stdout.take()?;
        let inp = child.stdin.take()?;
        Some(Self::new(out, inp, REMOTE_MAX_LINE))
    }

    pub fn into_parts(self) -> (LineReader, LineWriter) {
        (self.reader, self.writer)
    }
}

/// Named-pipe client handle opened with `FILE_FLAG_OVERLAPPED`; each read or
/// write is an overlapped operation waited on at once, so the two halves never
/// block each other.
mod overlapped {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT, WriteFile,
    };
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::CreateEventW;

    pub struct Half {
        pipe: Arc<OwnedHandle>,
        /// Manual-reset event of this half's operations (one at a time: `&mut self`).
        event: OwnedHandle,
    }

    fn event() -> io::Result<OwnedHandle> {
        // SAFETY: plain Win32 call; the handle is owned below.
        let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle we own.
        Ok(unsafe { OwnedHandle::from_raw_handle(h) })
    }

    pub fn open(path: &str) -> io::Result<(Half, Half)> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call. The daemon
        // may identify but not impersonate us.
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle we own.
        let pipe = Arc::new(unsafe { OwnedHandle::from_raw_handle(h) });
        Ok((
            Half {
                pipe: pipe.clone(),
                event: event()?,
            },
            Half {
                pipe,
                event: event()?,
            },
        ))
    }

    impl Half {
        /// Run one overlapped read or write to completion. `Ok(0)` when the
        /// other end closed (read).
        fn io(
            &mut self,
            op: impl FnOnce(*mut std::ffi::c_void, *mut OVERLAPPED) -> i32,
        ) -> io::Result<usize> {
            let h = self.pipe.as_raw_handle();
            // SAFETY: zeroed OVERLAPPED is valid; it lives on this frame until
            // the operation has completed (GetOverlappedResult waits).
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.event.as_raw_handle();
            if op(h, &mut ov) == 0 {
                // SAFETY: plain Win32 call.
                let err = unsafe { GetLastError() };
                if err == ERROR_BROKEN_PIPE {
                    return Ok(0);
                }
                if err != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(err as i32));
                }
            }
            let mut n = 0u32;
            // SAFETY: `ov` belongs to an operation started on `h`; bWait=TRUE.
            if unsafe { GetOverlappedResult(h, &ov, &mut n, 1) } == 0 {
                // SAFETY: plain Win32 call.
                let err = unsafe { GetLastError() };
                if err == ERROR_BROKEN_PIPE {
                    return Ok(0);
                }
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            Ok(n as usize)
        }
    }

    impl io::Read for Half {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_mut_ptr();
            // SAFETY: `buf` stays borrowed until the operation completed.
            self.io(|h, ov| unsafe { ReadFile(h, ptr, len, std::ptr::null_mut(), ov) })
        }
    }

    impl io::Write for Half {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_ptr();
            // SAFETY: `buf` stays borrowed until the operation completed.
            let n = self.io(|h, ov| unsafe { WriteFile(h, ptr, len, std::ptr::null_mut(), ov) })?;
            if n == 0 && !buf.is_empty() {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

/// Test support for security-01: a fake pipe server that reports how far
/// its client lets it impersonate.
#[cfg(all(test, windows))]
pub(crate) mod sqos_probe {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, RevertToSelf, TOKEN_QUERY, TokenImpersonationLevel,
    };
    use windows_sys::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, ImpersonateNamedPipeClient, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

    /// `SecurityIdentification` (`SECURITY_IMPERSONATION_LEVEL`).
    pub const IDENTIFICATION: i32 = 1;

    /// Serve `name` once: wait for a client, read one byte, impersonate it and
    /// return the impersonation level of the token we got (`-1` if none).
    pub fn serve_once(name: &str) -> std::thread::JoinHandle<i32> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated; default security.
        let h = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(h, INVALID_HANDLE_VALUE, "CreateNamedPipeW");
        let h = h as usize;
        std::thread::spawn(move || {
            let h = h as HANDLE;
            let mut level = -1i32;
            // SAFETY: plain Win32 calls on handles owned by this thread.
            unsafe {
                ConnectNamedPipe(h, std::ptr::null_mut());
                let mut b = [0u8; 1];
                let mut n = 0u32;
                ReadFile(h, b.as_mut_ptr(), 1, &mut n, std::ptr::null_mut());
                if ImpersonateNamedPipeClient(h) != 0 {
                    let mut tok: HANDLE = std::ptr::null_mut();
                    if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut tok) != 0 {
                        let mut len = 0u32;
                        GetTokenInformation(
                            tok,
                            TokenImpersonationLevel,
                            (&mut level as *mut i32).cast(),
                            4,
                            &mut len,
                        );
                        CloseHandle(tok);
                    }
                    RevertToSelf();
                }
                CloseHandle(h);
            }
            level
        })
    }

    pub fn pipe_name(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!(r"\\.\pipe\pod-sqos-{tag}-{}-{nanos}", std::process::id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// security-01: the plain-`File` pipe client and the overlapped client
    /// let the server identify, never impersonate, them.
    #[cfg(windows)]
    #[test]
    fn pipe_clients_allow_identification_only() {
        use super::sqos_probe::{IDENTIFICATION, pipe_name, serve_once};
        let name = pipe_name("file");
        let server = serve_once(&name);
        let mut f = open_pipe(&name).expect("open");
        f.write_all(b"x").unwrap();
        assert_eq!(server.join().unwrap(), IDENTIFICATION);

        let name = pipe_name("overlapped");
        let server = serve_once(&name);
        let (_r, mut w) = overlapped::open(&name).expect("open");
        w.write_all(b"x").unwrap();
        assert_eq!(server.join().unwrap(), IDENTIFICATION);
    }

    /// Hands out the data in fixed pieces, like a pipe delivering partial writes.
    struct Chunked {
        data: Vec<u8>,
        pos: usize,
        step: usize,
        reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Read for Chunked {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let n = self.step.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn chunked(data: &[u8], step: usize) -> Chunked {
        Chunked {
            data: data.to_vec(),
            pos: 0,
            step,
            reads: Default::default(),
        }
    }

    #[test]
    fn lines_split_across_reads_and_utf8_split_inside_a_char() {
        // "한" is 3 bytes; a step of 1 splits it across three reads.
        let data = "{\"a\":\"한글\"}\n{\"b\":2}\n".as_bytes();
        for step in [1, 2, 3, 5, 64] {
            let mut r = LineReader::new(chunked(data, step), 1024);
            assert_eq!(r.read_line().unwrap().as_deref(), Some("{\"a\":\"한글\"}"));
            assert_eq!(r.read_line().unwrap().as_deref(), Some("{\"b\":2}"));
            assert!(r.read_line().unwrap().is_none(), "clean EOF");
            assert!(r.read_line().unwrap().is_none(), "EOF stays EOF");
        }
    }

    #[test]
    fn eof_in_the_middle_of_a_line_is_an_error() {
        let mut r = LineReader::new(Cursor::new(b"{\"a\":1}\n{\"b\"".to_vec()), 1024);
        assert!(r.read_line().unwrap().is_some());
        assert!(matches!(r.read_line(), Err(FrameError::Truncated)));
    }

    #[test]
    fn oversized_line_is_refused_before_it_is_buffered() {
        // 10 MB without a newline, delivered 4 KiB at a time.
        let big = vec![b'x'; 10 * 1024 * 1024];
        let src = chunked(&big, 4096);
        let reads = src.reads.clone();
        let mut r = LineReader::new(src, 64 * 1024);
        assert!(matches!(r.read_line(), Err(FrameError::TooLarge(65536))));
        // It stopped right after the limit instead of reading everything.
        let n = reads.load(std::sync::atomic::Ordering::Relaxed);
        assert!(n <= 64 * 1024 / 4096 + 2, "read {n} chunks");
        // And stays broken: the rest of that line is still in the stream.
        assert!(matches!(r.read_line(), Err(FrameError::TooLarge(_))));
    }

    #[test]
    fn line_exactly_at_the_limit_is_fine() {
        let mut data = vec![b'y'; 100];
        data.push(b'\n');
        let mut r = LineReader::new(Cursor::new(data.clone()), 100);
        assert_eq!(r.read_line().unwrap().unwrap().len(), 100);
        let mut r = LineReader::new(Cursor::new(data), 99);
        assert!(matches!(r.read_line(), Err(FrameError::TooLarge(99))));
    }

    #[test]
    fn invalid_utf8_line_is_an_error_and_the_next_line_still_reads() {
        let mut r = LineReader::new(Cursor::new(b"\xff\xfe\n{}\n".to_vec()), 64);
        assert!(matches!(r.read_line(), Err(FrameError::NotUtf8)));
        assert_eq!(r.read_line().unwrap().as_deref(), Some("{}"));
    }

    #[test]
    fn writer_adds_newline_and_flushes() {
        let (pr, pw) = std::io::pipe().unwrap();
        let mut w = LineWriter::new(pw);
        w.write_line("{\"x\":1}").unwrap();
        drop(w);
        let mut r = LineReader::new(pr, 64);
        assert_eq!(r.read_line().unwrap().as_deref(), Some("{\"x\":1}"));
        assert!(r.read_line().unwrap().is_none());
    }
}
