//! `podd` — the Pod core daemon.
//!
//! ```text
//! podd [--pipe NAME|PATH] [--log FILE] [--state-dir DIR] [--holder EXE]
//!      [--worktrees-root DIR] [--retain-exited DURATION|off]
//!      [--review-queue-timeout DURATION|off] [--attach-queue-limit BYTES] [--reply-window MS]
//!                                        serve (normally started by `pod`)
//! podd schema                            print the JSON schema of the protocol
//! ```

use std::path::PathBuf;

use podd::endpoint;
use podd::server::{self, ServerConfig};

fn usage() -> ! {
    eprintln!(
        "usage: podd [--pipe NAME|PATH] [--log FILE] [--state-dir DIR] [--holder EXE] [--worktrees-root DIR] [--retain-exited DURATION|off] [--review-queue-timeout DURATION|off] [--attach-queue-limit BYTES] [--reply-window MS]\n       podd schema"
    );
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut pipe = None;
    let mut log: Option<PathBuf> = None;
    let mut state_dir: Option<PathBuf> = None;
    let mut holder: Option<PathBuf> = None;
    let mut worktrees_root: Option<PathBuf> = None;
    let mut retain: Option<Option<std::time::Duration>> = None;
    let mut review_timeout: Option<Option<std::time::Duration>> = None;
    let mut attach_limit: Option<usize> = None;
    let mut reply_window: Option<u64> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pipe" => pipe = Some(args.next().unwrap_or_else(|| usage())),
            "--log" => log = Some(args.next().unwrap_or_else(|| usage()).into()),
            "--state-dir" => state_dir = Some(args.next().unwrap_or_else(|| usage()).into()),
            "--holder" => holder = Some(args.next().unwrap_or_else(|| usage()).into()),
            "--worktrees-root" => {
                worktrees_root = Some(args.next().unwrap_or_else(|| usage()).into())
            }
            "--review-queue-timeout" => {
                let v = args.next().unwrap_or_else(|| usage());
                review_timeout =
                    Some(server::retention::parse_duration(&v).unwrap_or_else(|| usage()));
            }
            "--attach-queue-limit" => {
                let v = args.next().unwrap_or_else(|| usage());
                attach_limit = Some(v.parse().ok().filter(|&n| n > 0).unwrap_or_else(|| usage()));
            }
            "--reply-window" => {
                let v = args.next().unwrap_or_else(|| usage());
                reply_window = Some(v.trim().parse().unwrap_or_else(|_| usage()));
            }
            "--retain-exited" => {
                let v = args.next().unwrap_or_else(|| usage());
                retain = Some(server::retention::parse_duration(&v).unwrap_or_else(|| usage()));
            }
            "schema" => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&podd::protocol::schema()).unwrap()
                );
                return;
            }
            "--version" | "-V" => {
                println!("podd {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => usage(),
        }
    }
    if let Some(path) = &log {
        redirect_stderr(path);
    }
    let pipe = pipe
        .map(|p| endpoint::pipe_path(&p))
        .unwrap_or_else(endpoint::default_pipe);
    let mut config = ServerConfig::new(pipe);
    if let Some(d) = state_dir {
        config.state_dir = d;
    }
    if let Some(h) = holder {
        config.holder_exe = h;
    }
    if let Some(r) = retain {
        config.exited_retention = r;
    }
    if let Some(r) = review_timeout {
        config.review_queue_timeout = r;
    }
    if let Some(n) = attach_limit {
        config.attach_queue_limit = n;
    }
    if let Some(ms) = reply_window {
        config.reply_window_ms = ms;
    }
    if let Some(w) = worktrees_root {
        config.worktrees_root = std::path::absolute(&w).unwrap_or(w);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = rt.block_on(async {
        let first = match server::bind(&config) {
            Ok(x) => x,
            Err(e) => {
                server::log(&format!("{e}"));
                return 3;
            }
        };
        match server::serve(config, first.0, first.1).await {
            Ok(()) => 0,
            Err(e) => {
                server::log(&format!("fatal: {e}"));
                1
            }
        }
    });
    // Do not wait on PTY threads; terminals were already killed.
    std::process::exit(code);
}

/// Point fd 2 / STD_ERROR_HANDLE at an append-mode log file.
fn redirect_stderr(path: &PathBuf) {
    use std::os::windows::io::IntoRawHandle;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    unsafe extern "system" {
        fn SetStdHandle(n: u32, h: *mut core::ffi::c_void) -> i32;
    }
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    unsafe {
        SetStdHandle(STD_ERROR_HANDLE, file.into_raw_handle());
    }
}
