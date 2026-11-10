//! Local ConPTY session for the spike: raw bytes out, an [`InputSink`] in.
//!
//! This is what podd's attach connection will replace in pod-app: the view
//! only ever sees `feed(bytes)` and writes to a sink.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use pod_terminal_view::{InputSink, TermSize};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Bytes waiting for the UI; the reader stops reading past this (backpressure).
const MAX_PENDING: usize = 8 << 20;

pub enum PtyEvent {
    /// New output is waiting in [`LocalPty::take_output`].
    Output,
    Exit,
}

/// Remove launcher variables that contradict what this terminal advertises.
///
/// Agent tool shells (e.g. Claude Code's) export `NO_COLOR=1`; when pod-spike
/// is started from one, every child would inherit it and Node/Bun apps print
/// no SGR at all. Set `POD_SPIKE_KEEP_NO_COLOR=1` to keep it.
///
/// Call once at the very start of `main`, before any thread is spawned.
pub fn sanitize_child_env() {
    if std::env::var_os("POD_SPIKE_KEEP_NO_COLOR").is_none()
        && std::env::var_os("NO_COLOR").is_some()
    {
        // SAFETY: called first thing in `main`, single-threaded.
        unsafe { std::env::remove_var("NO_COLOR") };
    }
}

/// `vim` → `C:\...im.exe`: portable-pty's PATH search does not apply PATHEXT.
fn resolve_program(prog: &str) -> String {
    let p = std::path::Path::new(prog);
    if p.extension().is_some() || p.components().count() > 1 {
        return prog.to_string();
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{prog}{}", ext.to_ascii_lowercase()));
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    prog.to_string()
}

struct Shared {
    pending: Mutex<Vec<u8>>,
    /// An `Output` event is queued and not yet consumed.
    signaled: AtomicBool,
}

pub struct LocalPty {
    shared: Arc<Shared>,
    writer: std_mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl LocalPty {
    pub fn spawn(
        program: Option<(String, Vec<String>)>,
        tx: UnboundedSender<PtyEvent>,
    ) -> anyhow::Result<Arc<Self>> {
        let pair = native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let (prog, args) =
            program.unwrap_or_else(|| ("powershell.exe".into(), vec!["-NoLogo".into()]));
        let mut cmd = CommandBuilder::new(resolve_program(&prog));
        cmd.args(args);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if std::env::var_os("POD_SPIKE_KEEP_NO_COLOR").is_none() {
            cmd.env_remove("NO_COLOR");
        }
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let killer = child.clone_killer();

        let shared = Arc::new(Shared {
            pending: Mutex::new(Vec::new()),
            signaled: AtomicBool::new(false),
        });

        // Reader: PTY → pending buffer, one coalesced wakeup per UI drain.
        let mut reader = pair.master.try_clone_reader()?;
        let (s, t) = (shared.clone(), tx.clone());
        std::thread::Builder::new()
            .name("pty-read".into())
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    while s.pending.lock().unwrap().len() > MAX_PENDING {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            s.pending.lock().unwrap().extend_from_slice(&buf[..n]);
                            if !s.signaled.swap(true, Ordering::AcqRel)
                                && t.unbounded_send(PtyEvent::Output).is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                let _ = t.unbounded_send(PtyEvent::Exit);
            })?;

        // Writer: never block the UI thread on a full input pipe.
        let mut writer = pair.master.take_writer()?;
        let (wtx, wrx) = std_mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("pty-write".into())
            .spawn(move || {
                while let Ok(bytes) = wrx.recv() {
                    if writer
                        .write_all(&bytes)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;

        // ConPTY keeps the output pipe open after the child exits; report exit from wait().
        std::thread::Builder::new()
            .name("pty-wait".into())
            .spawn(move || {
                let _ = child.wait();
                let _ = tx.unbounded_send(PtyEvent::Exit);
            })?;

        Ok(Arc::new(Self {
            shared,
            writer: wtx,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
        }))
    }

    /// Everything read since the last call; re-arms the wakeup.
    pub fn take_output(&self) -> Vec<u8> {
        let out = std::mem::take(&mut *self.shared.pending.lock().unwrap());
        self.shared.signaled.store(false, Ordering::Release);
        out
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        let _ = self.killer.lock().unwrap().kill();
    }
}

/// The view's sink: a shared handle to the session.
pub struct PtySink(pub Arc<LocalPty>);

impl InputSink for PtySink {
    fn write(&self, bytes: &[u8]) {
        let _ = self.0.writer.send(bytes.to_vec());
    }

    fn resize(&self, size: TermSize) {
        let _ = self.0.master.lock().unwrap().resize(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.cols.saturating_mul(size.cell_width),
            pixel_height: size.rows.saturating_mul(size.cell_height),
        });
    }
}
