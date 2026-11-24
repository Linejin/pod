//! A bounded line-protocol session: a child process spoken to in lines on
//! stdin/stdout (JSON-RPC servers such as `codex app-server`), under the same
//! process-tree rules as [`super::run`].
//!
//! - The child starts suspended and hidden (`CREATE_SUSPENDED |
//!   CREATE_NO_WINDOW`), joins a kill-on-close Job Object and only then runs;
//!   a job that cannot be set up is an error and the child never runs.
//! - stderr is discarded; stdout is read on one thread into lines, each at
//!   most [`LineLimits::max_line`] bytes, all together at most
//!   [`LineLimits::max_total`]. More ends the session ([`RecvError::TooLarge`]).
//! - Every receive has a deadline. Dropping the session kills the whole tree
//!   (grandchildren included) and joins the reader.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

use super::{Tree, resume, settle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineLimits {
    /// Longest accepted line (bytes, without the newline).
    pub max_line: usize,
    /// All stdout bytes of the session.
    pub max_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    /// Nothing arrived before the deadline.
    TimedOut,
    /// stdout closed (the child exited).
    Closed,
    /// A line or the whole output went over its limit; the session is over.
    TooLarge,
}

enum Item {
    Line(String),
    TooLarge,
}

pub struct LineSession {
    child: Child,
    tree: Option<Tree>,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<Item>,
    reader: Option<std::thread::JoinHandle<()>>,
    over: bool,
}

impl LineSession {
    /// Start `cmd` (stdio and creation flags are set here).
    pub fn spawn(cmd: &mut Command, limits: LineLimits) -> std::io::Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
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
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (tx, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let Some(out) = stdout else { return };
            let mut r = BufReader::new(out);
            let mut total: u64 = 0;
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let n = match (&mut r)
                    .take(limits.max_line as u64 + 2)
                    .read_until(b'\n', &mut buf)
                {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                total += n as u64;
                let ended = buf.last() == Some(&b'\n');
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                if total > limits.max_total
                    || buf.len() > limits.max_line
                    || (!ended && n > limits.max_line)
                {
                    let _ = tx.send(Item::TooLarge);
                    return;
                }
                if tx
                    .send(Item::Line(String::from_utf8_lossy(&buf).into_owned()))
                    .is_err()
                {
                    return;
                }
            }
        });
        Ok(Self {
            child,
            tree,
            stdin,
            lines,
            reader: Some(reader),
            over: false,
        })
    }

    /// Write `line` and a newline.
    pub fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("stdin closed"))?;
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// The next stdout line, waiting until `deadline` at most.
    pub fn recv_line(&mut self, deadline: Instant) -> Result<String, RecvError> {
        if self.over {
            return Err(RecvError::TooLarge);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(left) {
            Ok(Item::Line(l)) => Ok(l),
            Ok(Item::TooLarge) => {
                self.over = true;
                self.kill();
                Err(RecvError::TooLarge)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(RecvError::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(RecvError::Closed),
        }
    }

    fn kill(&mut self) {
        match &self.tree {
            Some(t) => t.kill(),
            None => {
                let _ = self.child.kill();
            }
        }
    }
}

impl Drop for LineSession {
    fn drop(&mut self) {
        self.stdin = None;
        self.kill();
        let _ = self.child.wait();
        // The tree is gone, so the reader's pipe is broken; join it.
        self.tree = None;
        if let Some(h) = self.reader.take() {
            settle(vec![h]);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn limits() -> LineLimits {
        LineLimits {
            max_line: 64,
            max_total: 1024,
        }
    }

    fn ps(script: &str) -> Command {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        c
    }

    #[test]
    fn echoes_lines_in_order_and_times_out() {
        // Reads stdin lines and echoes them back with a prefix.
        let mut s = LineSession::spawn(
            &mut ps("$i=[Console]::In; while(($l=$i.ReadLine()) -ne $null){ [Console]::Out.WriteLine('re:'+$l); [Console]::Out.Flush() }"),
            limits(),
        )
        .unwrap();
        s.send_line("one").unwrap();
        s.send_line("two").unwrap();
        let d = Instant::now() + Duration::from_secs(30);
        assert_eq!(s.recv_line(d).unwrap(), "re:one");
        assert_eq!(s.recv_line(d).unwrap(), "re:two");
        assert_eq!(
            s.recv_line(Instant::now() + Duration::from_millis(200)),
            Err(RecvError::TimedOut)
        );
    }

    #[test]
    fn long_lines_and_too_much_output_end_the_session() {
        let mut s = LineSession::spawn(&mut ps("'x' * 500"), limits()).unwrap();
        let d = Instant::now() + Duration::from_secs(30);
        assert_eq!(s.recv_line(d), Err(RecvError::TooLarge));
        assert_eq!(s.recv_line(d), Err(RecvError::TooLarge), "stays over");
        let mut s = LineSession::spawn(&mut ps("1..400 | % { 'abcdefgh' }"), limits()).unwrap();
        let mut got = 0;
        let end = loop {
            match s.recv_line(d) {
                Ok(_) => got += 1,
                Err(e) => break e,
            }
        };
        assert_eq!(end, RecvError::TooLarge);
        assert!(got < 400 && got > 10, "{got}");
    }

    #[test]
    fn closed_when_the_child_exits_and_drop_kills_the_tree() {
        let mut s = LineSession::spawn(&mut ps("'bye'"), limits()).unwrap();
        let d = Instant::now() + Duration::from_secs(30);
        assert_eq!(s.recv_line(d).unwrap(), "bye");
        assert_eq!(s.recv_line(d), Err(RecvError::Closed));
        // A child that starts a long-lived grandchild: dropping the session
        // ends both (no orphan).
        let mut s = LineSession::spawn(
            &mut ps("Start-Process -NoNewWindow ping -ArgumentList '-n','60','127.0.0.1'; 'started'; Start-Sleep 60"),
            limits(),
        )
        .unwrap();
        assert_eq!(s.recv_line(d).unwrap(), "started");
        let pid = s.child.id();
        drop(s);
        let alive = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("@(Get-CimInstance Win32_Process | Where-Object {{ $_.ParentProcessId -eq {pid} -or $_.ProcessId -eq {pid} }}).Count"),
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&alive.stdout).trim(), "0");
    }
}
