//! Pod leaf crate for running external processes. It depends on std and the
//! Windows API only; product crates (forge, editor, podd) use it, never the
//! other way round.
//!
//! - [`bounded`] runs one-shot child processes (forge's git and gh, podd's
//!   guarded git, group scripts and repo config commands, the editor's LSP
//!   probe). Consumers name it `pod_process::bounded`; the old
//!   `pod_forge::bounded` re-export is gone (rf2-bounds).
//! - [`git`] clears the `GIT_*` variables that would point git at another
//!   repository, the one environment rule podd and pod-forge share.

pub mod git;

pub mod bounded {
    //! Bounded child processes: one way to run an external command with a time
    //! limit that holds for the whole call (orch-03, security-02/03).
    //!
    //! - **Process tree.** On Windows the child goes into a Job Object with
    //!   `KILL_ON_JOB_CLOSE`; a timeout, cancel or output overflow terminates the
    //!   job, so grandchildren that inherited the output pipes die too and the
    //!   pipes close. Leftovers are also killed when the run ends normally
    //!   (after a short grace for pipes to close on their own). The child is
    //!   created suspended (`CREATE_SUSPENDED | CREATE_NO_WINDOW`; the creation
    //!   flags are set here), assigned to the job and only then resumed, so
    //!   nothing it starts can escape. A job that cannot be created or assigned
    //!   is an error: the child is killed before it ran and `run` returns `Err`
    //!   (never an unbounded run).
    //! - **Deadline.** `timeout` covers waiting *and* draining: after the
    //!   deadline the job is killed, readers get a short kill grace, and any
    //!   reader still blocked has its read cancelled (`CancelSynchronousIo`).
    //!   Reader and stdin threads are joined before `run` returns; only one
    //!   that ignores the cancel for a further second is left behind.
    //! - **Output.** Each stream is read as it arrives into a bounded sink:
    //!   [`Keep::Tail`] keeps the last N bytes, [`Keep::Cap`] stops the run with
    //!   [`End::TooLarge`] once more than N bytes arrive. Nothing reads into an
    //!   unbounded buffer.

    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    /// How much of a stream to keep.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Keep {
        /// The last N bytes; older output is dropped.
        Tail(usize),
        /// At most N bytes; more ends the run with [`End::TooLarge`].
        Cap(usize),
    }

    /// What to run with.
    #[derive(Debug, Clone)]
    pub struct Options {
        /// Whole-call limit: waiting for the exit plus draining the pipes.
        pub timeout: Duration,
        pub stdout: Keep,
        /// Ignored when `merge` is set (stderr goes into the stdout sink).
        pub stderr: Keep,
        /// One sink for both streams, interleaved in arrival order.
        pub merge: bool,
        /// Written on a separate thread (a large body never blocks the wait).
        pub stdin: Option<Vec<u8>>,
        /// After the child exits, how long inherited pipes may stay open before
        /// the leftovers are killed.
        pub after_exit_grace: Duration,
    }

    impl Options {
        pub fn new(timeout: Duration, keep: Keep) -> Self {
            Self {
                timeout,
                stdout: keep,
                stderr: keep,
                merge: false,
                stdin: None,
                after_exit_grace: Duration::from_secs(2),
            }
        }
    }

    /// How the run ended.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum End {
        Exited {
            code: Option<i32>,
            success: bool,
        },
        TimedOut,
        Cancelled,
        /// A [`Keep::Cap`] stream went over its limit; the tree was killed.
        TooLarge {
            stream: &'static str,
            limit: usize,
        },
    }

    /// Bytes kept from one stream.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Captured {
        pub bytes: Vec<u8>,
        /// Everything the stream produced (kept or not).
        pub total: u64,
        /// Older bytes were dropped ([`Keep::Tail`]) or the cap was hit.
        pub truncated: bool,
    }

    impl Captured {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.bytes).into_owned()
        }
    }

    #[derive(Debug, Clone)]
    pub struct Outcome {
        pub end: End,
        pub stdout: Captured,
        /// Empty with [`Options::merge`].
        pub stderr: Captured,
        pub elapsed: Duration,
    }

    /// How long readers get after the tree was killed.
    const KILL_GRACE: Duration = Duration::from_millis(1500);
    const POLL: Duration = Duration::from_millis(20);

    struct Sink {
        keep: Keep,
        name: &'static str,
        buf: Vec<u8>,
        total: u64,
        truncated: bool,
        overflow: bool,
    }

    impl Sink {
        fn new(keep: Keep, name: &'static str) -> Arc<Mutex<Sink>> {
            Arc::new(Mutex::new(Sink {
                keep,
                name,
                buf: Vec::new(),
                total: 0,
                truncated: false,
                overflow: false,
            }))
        }

        fn push(&mut self, chunk: &[u8]) {
            self.total += chunk.len() as u64;
            match self.keep {
                Keep::Tail(n) => {
                    self.buf.extend_from_slice(chunk);
                    // Amortized: trim once the buffer doubles.
                    if self.buf.len() > n.saturating_mul(2).max(1) {
                        let cut = self.buf.len() - n;
                        self.buf.drain(..cut);
                        self.truncated = true;
                    }
                }
                Keep::Cap(n) => {
                    let room = n.saturating_sub(self.buf.len());
                    self.buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
                    if self.total > n as u64 {
                        self.truncated = true;
                        self.overflow = true;
                    }
                }
            }
        }

        fn take(&mut self) -> Captured {
            if let Keep::Tail(n) = self.keep
                && self.buf.len() > n
            {
                let cut = self.buf.len() - n;
                self.buf.drain(..cut);
                self.truncated = true;
            }
            Captured {
                bytes: std::mem::take(&mut self.buf),
                total: self.total,
                truncated: self.truncated,
            }
        }
    }

    fn lock(s: &Mutex<Sink>) -> std::sync::MutexGuard<'_, Sink> {
        s.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reader(
        pipe: Option<Box<dyn Read + Send>>,
        sink: Arc<Mutex<Sink>>,
        done: mpsc::Sender<()>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            if let Some(mut r) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = r.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    let mut s = lock(&sink);
                    s.push(&chunk[..n]);
                    if s.overflow {
                        // Stop reading: the run is being stopped anyway.
                        break;
                    }
                }
            }
            let _ = done.send(());
        })
    }

    /// How long `run` keeps cancelling blocked helper threads before leaving
    /// them behind.
    const SETTLE: Duration = Duration::from_secs(1);

    /// Joins the reader/stdin threads. The tree is gone by now, so their pipes
    /// are broken; a thread still blocked (a pipe held outside the job) gets its
    /// synchronous I/O cancelled until it ends or `SETTLE` passes. Returns how
    /// many were left running.
    fn settle(threads: Vec<std::thread::JoinHandle<()>>) -> usize {
        let until = Instant::now() + SETTLE;
        let mut left = 0;
        for h in threads {
            while !h.is_finished() && Instant::now() < until {
                cancel_io(&h);
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                left += 1;
            }
        }
        #[cfg(test)]
        LAST_LEFT.with(|c| c.set(left));
        left
    }

    #[cfg(windows)]
    fn cancel_io(h: &std::thread::JoinHandle<()>) {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: the thread handle stays valid while the JoinHandle lives.
        unsafe { windows_sys::Win32::System::IO::CancelSynchronousIo(h.as_raw_handle() as _) };
    }

    #[cfg(not(windows))]
    fn cancel_io(_: &std::thread::JoinHandle<()>) {}

    #[cfg(test)]
    thread_local! {
        /// Test hook: the next job assignment on this thread fails.
        static FAIL_ASSIGN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Helper threads the last `run` on this thread left running.
        static LAST_LEFT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    }

    /// Runs `cmd` (its stdio and, on Windows, its creation flags are set here)
    /// within `opts`. `Err` when the process could not start or could not be
    /// put in its process tree; everything else is an [`Outcome`]. Setting
    /// `cancel` stops the run (and its tree) within one poll interval.
    pub fn run(
        cmd: &mut Command,
        opts: &Options,
        cancel: Option<&AtomicBool>,
    ) -> std::io::Result<Outcome> {
        let start = Instant::now();
        let deadline = start + opts.timeout;
        cmd.stdin(if opts.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        }
        let tree = Tree::new()?;
        let mut child = cmd.spawn()?;
        if let Some(t) = &tree
            && let Err(e) = t.assign(&child).and_then(|()| resume(child.id()))
        {
            // Never run outside the tree: the child is still suspended (or
            // already in the job) here.
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let mut threads = Vec::with_capacity(3);
        if let (Some(text), Some(mut pipe)) = (opts.stdin.clone(), child.stdin.take()) {
            threads.push(std::thread::spawn(move || {
                let _ = pipe.write_all(&text);
            }));
        }
        let out = Sink::new(opts.stdout, "stdout");
        let err = if opts.merge {
            out.clone()
        } else {
            Sink::new(opts.stderr, "stderr")
        };
        let (tx, readers_done) = mpsc::channel();
        threads.push(reader(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            out.clone(),
            tx.clone(),
        ));
        threads.push(reader(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            err.clone(),
            tx,
        ));
        let mut pending = 2usize;

        let overflowed = || {
            for s in [&out, &err] {
                let s = lock(s);
                if s.overflow
                    && let Keep::Cap(limit) = s.keep
                {
                    return Some(End::TooLarge {
                        stream: s.name,
                        limit,
                    });
                }
            }
            None
        };
        let stop = |child: &mut Child| {
            match &tree {
                Some(t) => t.kill(),
                None => {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        };

        // 1. Wait for the exit (or a reason to stop).
        let mut end = loop {
            if let Some(e) = overflowed() {
                stop(&mut child);
                break e;
            }
            if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                stop(&mut child);
                break End::Cancelled;
            }
            match child.try_wait() {
                Ok(Some(st)) => {
                    break End::Exited {
                        code: st.code(),
                        success: st.success(),
                    };
                }
                Ok(None) if Instant::now() >= deadline => {
                    stop(&mut child);
                    break End::TimedOut;
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => {
                    stop(&mut child);
                    break End::Exited {
                        code: None,
                        success: false,
                    };
                }
            }
        };

        // 2. Drain: pipes held by leftovers get a grace (never past the
        //    deadline), then the tree is killed and readers get a kill grace.
        let exited = matches!(end, End::Exited { .. });
        let drain_until = if exited {
            (Instant::now() + opts.after_exit_grace).min(deadline)
        } else {
            Instant::now()
        };
        while pending > 0
            && readers_done
                .recv_timeout(drain_until.saturating_duration_since(Instant::now()))
                .is_ok()
        {
            pending -= 1;
        }
        if pending > 0 {
            if let Some(t) = &tree {
                t.kill();
            }
            let until = Instant::now() + KILL_GRACE;
            while pending > 0
                && readers_done
                    .recv_timeout(until.saturating_duration_since(Instant::now()))
                    .is_ok()
            {
                pending -= 1;
            }
            if exited && Instant::now() >= deadline {
                end = End::TimedOut;
            }
        }
        if exited && let Some(e) = overflowed() {
            end = e;
        }
        // Dropping the tree kills anything still in it; then the helper
        // threads end (their pipes are broken) and are joined.
        drop(tree);
        settle(threads);
        let stdout = lock(&out).take();
        let stderr = if opts.merge {
            Captured::default()
        } else {
            lock(&err).take()
        };
        Ok(Outcome {
            end,
            stdout,
            stderr,
            elapsed: start.elapsed(),
        })
    }

    /// Keeps the last `max` bytes of `s` on a char boundary, with a leading `…`
    /// when something was cut.
    pub fn tail_str(s: &str, max: usize) -> String {
        if s.len() <= max {
            return s.to_string();
        }
        let mut cut = s.len() - max;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        format!("…{}", &s[cut..])
    }

    /// A kill-on-close Job Object holding the child and everything it starts.
    #[cfg(windows)]
    struct Tree(windows_sys::Win32::Foundation::HANDLE);

    #[cfg(windows)]
    impl Tree {
        fn new() -> std::io::Result<Option<Tree>> {
            use windows_sys::Win32::System::JobObjects::*;
            // SAFETY: plain Win32 calls; the handle is owned by `Tree`.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(job_error("create"));
                }
                let t = Tree(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                );
                if ok == 0 {
                    return Err(job_error("configure"));
                }
                Ok(Some(t))
            }
        }

        fn assign(&self, child: &Child) -> std::io::Result<()> {
            use std::os::windows::io::AsRawHandle;
            #[cfg(test)]
            if FAIL_ASSIGN.with(|c| c.replace(false)) {
                return Err(std::io::Error::other("job assign failed (injected)"));
            }
            // SAFETY: both handles are valid for the call.
            let ok = unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                    self.0,
                    child.as_raw_handle() as _,
                )
            };
            if ok == 0 {
                return Err(job_error("assign"));
            }
            Ok(())
        }

        fn kill(&self) {
            // SAFETY: valid job handle.
            unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) };
        }
    }

    #[cfg(windows)]
    fn job_error(step: &str) -> std::io::Error {
        let os = std::io::Error::last_os_error();
        std::io::Error::new(os.kind(), format!("process tree (job {step}): {os}"))
    }

    /// Resumes the (single, suspended) main thread of process `pid`.
    #[cfg(windows)]
    fn resume(pid: u32) -> std::io::Result<()> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
        use windows_sys::Win32::System::Threading::{
            OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        };
        // SAFETY: plain Win32 calls on handles opened and closed here; the
        // THREADENTRY32 is zeroed with its size set as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut te: THREADENTRY32 = std::mem::zeroed();
            te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut ok = Thread32First(snap, &mut te);
            while ok != 0 {
                if te.th32OwnerProcessID == pid {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID);
                    if !t.is_null() {
                        if ResumeThread(t) != u32::MAX {
                            resumed += 1;
                        }
                        CloseHandle(t);
                    }
                }
                ok = Thread32Next(snap, &mut te);
            }
            CloseHandle(snap);
            if resumed == 0 {
                return Err(std::io::Error::other("could not resume the new process"));
            }
            Ok(())
        }
    }

    #[cfg(not(windows))]
    fn resume(_: u32) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(windows)]
    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: we own the handle; closing kills what is left in the job.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }

    /// Elsewhere only the direct child is killed.
    #[cfg(not(windows))]
    struct Tree;

    #[cfg(not(windows))]
    impl Tree {
        fn new() -> std::io::Result<Option<Tree>> {
            Ok(None)
        }
        fn assign(&self, _: &Child) -> std::io::Result<()> {
            Ok(())
        }
        fn kill(&self) {}
    }

    #[cfg(all(test, windows))]
    mod tests {
        use super::*;

        fn cmd(line: &str) -> Command {
            let mut c = Command::new("cmd");
            c.args(["/d", "/c", line]);
            c
        }

        /// The process (if any) whose command line contains `marker`.
        fn alive(marker: &str) -> bool {
            let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("@(Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*{marker}*' -and $_.Name -eq 'ping.exe' }}).Count")])
            .output()
            .unwrap();
            String::from_utf8_lossy(&out.stdout).trim() != "0"
        }

        #[test]
        fn exit_code_and_both_streams() {
            let o = run(
                &mut cmd("echo out & echo err 1>&2 & exit 3"),
                &Options::new(Duration::from_secs(20), Keep::Tail(1024)),
                None,
            )
            .unwrap();
            assert_eq!(
                o.end,
                End::Exited {
                    code: Some(3),
                    success: false
                }
            );
            assert!(o.stdout.text().contains("out"), "{o:?}");
            assert!(o.stderr.text().contains("err"), "{o:?}");
            let mut opts = Options::new(Duration::from_secs(20), Keep::Tail(1024));
            opts.merge = true;
            let o = run(&mut cmd("echo out & echo err 1>&2"), &opts, None).unwrap();
            let t = o.stdout.text();
            assert!(t.contains("out") && t.contains("err"), "{o:?}");
        }

        /// The parent exits at once; a grandchild keeps the pipes open far past
        /// the deadline. The call still returns by the deadline and the
        /// grandchild is gone.
        #[test]
        fn grandchild_holding_pipes_is_killed_at_the_deadline() {
            let marker = "127.0.0.42";
            let mut opts = Options::new(Duration::from_secs(3), Keep::Tail(1024));
            opts.after_exit_grace = Duration::from_secs(60);
            let t = Instant::now();
            let o = run(
                &mut cmd(&format!("start /b ping -n 120 {marker} & echo parent-done")),
                &opts,
                None,
            )
            .unwrap();
            assert!(
                t.elapsed() < Duration::from_secs(8),
                "returned at the deadline: {:?}",
                t.elapsed()
            );
            assert_eq!(
                o.end,
                End::TimedOut,
                "pipes still open at the deadline: {o:?}"
            );
            assert!(
                o.stdout.text().contains("parent-done"),
                "output so far is kept: {o:?}"
            );
            assert!(!alive(marker), "the grandchild was killed with the tree");
        }

        /// The parent itself runs past the deadline with a child of its own:
        /// both die.
        #[test]
        fn timeout_kills_the_whole_tree() {
            let marker = "127.0.0.43";
            let t = Instant::now();
            let o = run(
                &mut cmd(&format!(
                    "start /b ping -n 120 {marker} & ping -n 120 127.0.0.1 >NUL"
                )),
                &Options::new(Duration::from_millis(1500), Keep::Tail(64)),
                None,
            )
            .unwrap();
            assert_eq!(o.end, End::TimedOut);
            assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
            assert!(!alive(marker), "the grandchild was killed with the tree");
        }

        /// After a normal exit, leftovers holding the pipes get only the grace.
        #[test]
        fn leftovers_after_exit_get_the_grace_then_die() {
            let marker = "127.0.0.44";
            let mut opts = Options::new(Duration::from_secs(30), Keep::Tail(1024));
            opts.after_exit_grace = Duration::from_millis(500);
            let t = Instant::now();
            let o = run(
                &mut cmd(&format!("start /b ping -n 120 {marker} & echo done")),
                &opts,
                None,
            )
            .unwrap();
            assert_eq!(
                o.end,
                End::Exited {
                    code: Some(0),
                    success: true
                },
                "{o:?}"
            );
            assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
            assert!(!alive(marker));
        }

        /// Output above a small limit: the tail keeps only the end; the cap
        /// stops the run with an explicit error. Memory stays bounded either way.
        #[test]
        fn output_above_the_limit() {
            let flood = "for /L %i in (1,1,20000) do @echo line-%i";
            let o = run(
                &mut cmd(flood),
                &Options::new(Duration::from_secs(60), Keep::Tail(256)),
                None,
            )
            .unwrap();
            assert!(matches!(o.end, End::Exited { success: true, .. }), "{o:?}");
            assert!(
                o.stdout.bytes.len() <= 256 && o.stdout.truncated,
                "{}",
                o.stdout.bytes.len()
            );
            assert!(o.stdout.total > 100_000, "{}", o.stdout.total);
            assert!(
                o.stdout.text().trim_end().ends_with("line-20000"),
                "{:?}",
                o.stdout.text()
            );

            let t = Instant::now();
            let o = run(
                &mut cmd("for /L %i in (1,1,1000000) do @echo line-%i"),
                &Options::new(Duration::from_secs(60), Keep::Cap(4096)),
                None,
            )
            .unwrap();
            assert_eq!(
                o.end,
                End::TooLarge {
                    stream: "stdout",
                    limit: 4096
                },
                "{o:?}"
            );
            assert!(o.stdout.bytes.len() <= 4096);
            assert!(
                t.elapsed() < Duration::from_secs(20),
                "stopped early: {:?}",
                t.elapsed()
            );
        }

        #[test]
        fn cancel_and_stdin_and_missing_program() {
            let flag = AtomicBool::new(true);
            let o = run(
                &mut cmd("ping -n 30 127.0.0.1 >NUL"),
                &Options::new(Duration::from_secs(30), Keep::Tail(64)),
                Some(&flag),
            )
            .unwrap();
            assert_eq!(o.end, End::Cancelled);
            let mut opts = Options::new(Duration::from_secs(20), Keep::Cap(1024));
            opts.stdin = Some(b"from-stdin\r\n".to_vec());
            let o = run(&mut cmd("findstr from"), &opts, None).unwrap();
            assert!(o.stdout.text().contains("from-stdin"), "{o:?}");
            assert!(run(&mut Command::new("pod-no-such-tool-xyz"), &opts, None).is_err());
        }

        /// A job that cannot take the child is an error, and the child never
        /// ran (it was still suspended): no unbounded run.
        #[test]
        fn job_assign_failure_is_an_error_and_nothing_runs() {
            let dir =
                std::env::temp_dir().join(format!("pod-bounded-assign-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let mark = dir.join("ran.txt");
            let _ = std::fs::remove_file(&mark);
            FAIL_ASSIGN.with(|c| c.set(true));
            let r = run(
                &mut cmd(&format!("echo ran> \"{}\"", mark.display())),
                &Options::new(Duration::from_secs(20), Keep::Tail(64)),
                None,
            );
            let e = r.expect_err("assign failure must fail the run");
            assert!(e.to_string().contains("injected"), "{e}");
            std::thread::sleep(Duration::from_millis(500));
            assert!(!mark.exists(), "the child ran outside the tree");
            let _ = std::fs::remove_dir_all(&dir);
            // The hook is one-shot: the next run works.
            let o = run(
                &mut cmd("echo ok"),
                &Options::new(Duration::from_secs(20), Keep::Tail(64)),
                None,
            )
            .unwrap();
            assert!(o.stdout.text().contains("ok"), "{o:?}");
        }

        /// A grandchild started in the first instant is in the job too (the
        /// child ran only after the assign): it dies with the tree even when
        /// the parent exits at once and no grace is given.
        #[test]
        fn grandchild_spawned_immediately_is_in_the_tree() {
            let marker = "127.0.0.46";
            let mut opts = Options::new(Duration::from_secs(30), Keep::Tail(256));
            opts.after_exit_grace = Duration::ZERO;
            let t = Instant::now();
            let o = run(
                &mut cmd(&format!("start /b ping -n 120 {marker}")),
                &opts,
                None,
            )
            .unwrap();
            assert!(matches!(o.end, End::Exited { .. }), "{o:?}");
            assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
            assert!(!alive(marker), "the grandchild escaped the tree");
            assert_eq!(LAST_LEFT.with(|c| c.get()), 0, "helper threads joined");
        }

        /// A large stdin body the child never reads: the writer thread is
        /// blocked until the tree dies, and is joined before `run` returns.
        #[test]
        fn blocked_stdin_writer_does_not_outlive_the_call() {
            let mut opts = Options::new(Duration::from_millis(1500), Keep::Tail(64));
            opts.stdin = Some(vec![b'x'; 8 << 20]);
            let t = Instant::now();
            let o = run(&mut cmd("ping -n 30 127.0.0.1 >NUL"), &opts, None).unwrap();
            assert_eq!(o.end, End::TimedOut, "{o:?}");
            assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
            assert_eq!(
                LAST_LEFT.with(|c| c.get()),
                0,
                "stdin/reader threads joined"
            );
        }

        #[test]
        fn tail_str_cuts_on_a_char_boundary() {
            assert_eq!(tail_str("abc", 5), "abc");
            assert_eq!(tail_str("가나다", 4), "…다");
        }
    }
}
