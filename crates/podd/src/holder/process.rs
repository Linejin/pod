//! The holder process: the PTY child and its VT state, the files next to
//! the record (the record itself, `.log`, `.err` on a failed start), and the
//! threads around the child: the output reader and the exit watcher.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::connection::{self, Conn};
use super::input::{self, InputItem, InputQueue};
use super::{OUTPUT_CHUNK, REPLY_BYTE_CAP, REPLY_COUNT_CAP};
use crate::holder_proto::{ExitInfo, Frame, HolderRecord, HolderSpec};
use crate::pty::Pty;
use crate::vt::{OSC7501_REPLY, Osc7501Query, Vt};
use crate::winsec::UserOnly;

pub(super) struct State {
    pub(super) vt: Vt,
    pub(super) exited: Option<ExitInfo>,
    pub(super) killed: bool,
    /// The connected daemon.
    pub(super) sub: Option<Arc<Conn>>,
}

pub(super) struct Holder {
    pub(super) spec: HolderSpec,
    pub(super) pty: Arc<Pty>,
    pub(super) state: Mutex<State>,
    exit_code: Mutex<Option<i32>>,
    last_output: Mutex<Instant>,
    pub(super) input: Arc<InputQueue>,
    log: Mutex<Option<File>>,
    pub(super) sa: UserOnly,
}

impl Holder {
    pub(super) fn log(&self, msg: &str) {
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "[pod-holder {} {}] {msg}", self.spec.id, crate::now_ms());
        }
    }

    pub(super) fn record_path(&self) -> PathBuf {
        PathBuf::from(&self.spec.record_path)
    }
}

pub(super) fn sibling(record: &Path, ext: &str) -> PathBuf {
    record.with_extension(ext)
}

/// Runs the holder for `spec` (its file already removed) until `Release`.
/// Returns the process exit code of a failed start.
pub(super) fn run(spec: HolderSpec) -> i32 {
    let record = PathBuf::from(&spec.record_path);
    let fail = |msg: String| {
        let _ = std::fs::write(sibling(&record, "err"), &msg);
        1
    };

    let sa = match UserOnly::new() {
        Ok(sa) => sa,
        Err(e) => return fail(format!("security descriptor: {e}")),
    };
    let (pty, output) = match Pty::spawn(
        &spec.argv,
        Path::new(&spec.cwd),
        &spec.env,
        spec.cols,
        spec.rows,
    ) {
        Ok(p) => p,
        Err(e) => return fail(format!("{e}")),
    };
    let pty = Arc::new(pty);
    // Tests only (debug builds): a holder that is slow to serve its pipe,
    // so a stop can land deterministically inside `terminal.run`.
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("POD_HOLDER_TEST_START_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_millis(ms.min(30_000)));
    }

    // All writes to the child's input go through one thread (never block
    // tokio); it starts once the holder exists (it acknowledges input).
    let (input_tx, input_rx) = std_mpsc::channel::<InputItem>();
    let input = Arc::new(InputQueue::new(input_tx));
    let reply: crate::vt::ReplySink = {
        let input = input.clone();
        Arc::new(move |b: &[u8]| {
            input.push_reply(b.to_vec());
        })
    };

    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(sibling(&record, "log"))
        .ok();
    let holder = Arc::new(Holder {
        state: Mutex::new(State {
            vt: Vt::new(spec.cols, spec.rows, Some(reply)),
            exited: None,
            killed: false,
            sub: None,
        }),
        spec,
        pty,
        exit_code: Mutex::new(None),
        last_output: Mutex::new(Instant::now()),
        input,
        log: Mutex::new(log),
        sa,
    });
    input::spawn_writer(holder.clone(), input_rx);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            holder.pty.kill_tree(1);
            return fail(format!("runtime: {e}"));
        }
    };
    let first = {
        let _guard = rt.enter();
        match connection::first_instance(&holder) {
            Ok(s) => s,
            Err(e) => {
                holder.pty.kill_tree(1);
                return fail(format!(
                    "cannot create holder pipe {}: {e}",
                    holder.spec.pipe
                ));
            }
        }
    };

    let rec = HolderRecord {
        id: holder.spec.id.clone(),
        name: holder.spec.name.clone(),
        pipe: holder.spec.pipe.clone(),
        holder_pid: std::process::id(),
        child_pid: holder.pty.pid(),
        created_ms: holder.spec.created_ms,
    };
    let tmp = sibling(&record, "json.tmp");
    if let Err(e) = std::fs::write(&tmp, serde_json::to_vec_pretty(&rec).unwrap())
        .and_then(|_| std::fs::rename(&tmp, &record))
    {
        holder.pty.kill_tree(1);
        return fail(format!("cannot write record {}: {e}", record.display()));
    }
    holder.log(&format!(
        "started {:?} pid {} on {}",
        holder.spec.argv,
        holder.pty.pid(),
        holder.spec.pipe
    ));

    spawn_output_reader(holder.clone(), output);
    spawn_exit_watcher(holder.clone());

    rt.block_on(connection::accept_loop(holder, first));
    0
}

/// PTY output reader: feeds the VT, answers OSC 7501 queries in stream
/// order and sends the output to the connected daemon (waiting while its
/// queue is over the budget). At end of output: the exit.
fn spawn_output_reader(h: Arc<Holder>, mut output: File) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; OUTPUT_CHUNK];
        let mut osc7501 = Osc7501Query::default();
        loop {
            match output.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    h.wait_output_room(n);
                    *h.last_output.lock().unwrap() = Instant::now();
                    let chunk = &buf[..n];
                    let mut st = h.state.lock().unwrap();
                    let seq = st.vt.seq();
                    // Feed up to each OSC 7501 query and answer it right
                    // there, so replies leave in stream order (before the
                    // DA1 reply that Claude uses as its "no answer" fence).
                    let mut done = 0;
                    for end in osc7501.scan(chunk) {
                        st.vt.feed(&chunk[done..end]);
                        h.input.push_reply(OSC7501_REPLY.to_vec());
                        done = end;
                    }
                    st.vt.feed(&chunk[done..]);
                    if let Some(total) = h.input.take_drop_notice() {
                        h.log(&format!(
                            "terminal query replies dropped: over {REPLY_BYTE_CAP} bytes / {REPLY_COUNT_CAP} replies waiting for the program ({total} in total)"
                        ));
                    }
                    Holder::send(
                        &mut st,
                        &Frame::Output {
                            seq,
                            data: chunk.to_vec(),
                        },
                    );
                }
            }
        }
        // End of output: the pseudo console was closed after the child exited.
        let mut st = h.state.lock().unwrap();
        let info = ExitInfo {
            seq: st.vt.seq(),
            exit_code: *h.exit_code.lock().unwrap(),
            killed: st.killed,
            exited_ms: crate::now_ms(),
        };
        st.exited = Some(info.clone());
        Holder::send(&mut st, &Frame::Exited(info.clone()));
        drop(st);
        h.log(&format!(
            "exited code {:?} killed {}",
            info.exit_code, info.killed
        ));
    });
}

/// Child exit watcher: let the last output drain, then close the console
/// so the reader sees end-of-file.
fn spawn_exit_watcher(h: Arc<Holder>) {
    std::thread::spawn(move || {
        let code = h.pty.wait();
        *h.exit_code.lock().unwrap() = code;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3)
            && h.last_output.lock().unwrap().elapsed() < Duration::from_millis(150)
        {
            std::thread::sleep(Duration::from_millis(30));
        }
        h.pty.close_console();
    });
}
