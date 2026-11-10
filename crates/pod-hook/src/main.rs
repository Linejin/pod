//! `pod-hook` — relay an agent hook event to podd.
//!
//! ```text
//! pod-hook claude|codex <Event>     # invoked by the agent; hook JSON on stdin
//! pod-hook install --dry-run [...]  # print the hooks config Pod would install
//! ```
//!
//! Hook mode contract: ALWAYS prints `{}` and exits 0, never blocks the agent
//! for more than ~1 s, never changes the agent's decision. Without
//! `POD_TERMINAL_ID` / `POD_SOCKET` it exits immediately (harmless outside Pod).
//! It never opens a window: on Windows it is a GUI-subsystem executable, so
//! the OS never allocates a console for it, however the agent spawns it (even
//! from a detached process, which would make a console exe pop up a new
//! console window). stdin/stdout still work: agents pass pipes as the
//! standard handles. No child processes, no windows. `install` / `--help`
//! attach to the parent console when stdout is not redirected.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pod_status::hook::{
    ENV_HOOK_TOKEN, ENV_SOCKET, ENV_TERMINAL_ID, HookAgent, HookMessage, MAX_HOOK_LINE,
    TRUNCATED_KEY,
};
use pod_status::install::{self, HookCommandSpec, InstallScope};
use serde_json::Value;

/// Total time budget for reading stdin + delivering the message.
const DEADLINE: Duration = Duration::from_millis(1000);
/// Part of the budget for reading stdin.
const STDIN_BUDGET: Duration = Duration::from_millis(500);
/// Larger payloads are forwarded as a `pod_truncated` marker (event name
/// still delivered). The whole line is capped separately (`to_wire_line`).
const STDIN_LIMIT: u64 = 4 * 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        args.first().map(String::as_str),
        Some("install" | "-h" | "--help" | "help" | "--version" | "-V")
    ) {
        use_parent_console();
    }
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        // `pod hooks status` compares the installed copy with this build.
        println!("pod-hook {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.first().map(String::as_str) == Some("install") {
        std::process::exit(install_cli(&args[1..]));
    }
    if matches!(
        args.first().map(String::as_str),
        Some("-h" | "--help" | "help")
    ) {
        println!(
            "usage: pod-hook claude|codex <Event>   (hook JSON on stdin)\n       pod-hook install --dry-run [--agent claude|codex|all] [--home DIR | --claude-config-dir DIR --codex-home DIR] [--exe PATH] [--uninstall] [--block-only]"
        );
        return;
    }
    let start = Instant::now();
    // Print first: the agent gets its answer no matter what happens next.
    {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(b"{}\n");
        let _ = out.flush();
    }
    let _ = std::panic::catch_unwind(|| relay(&args, start));
    exit_now();
}

/// Exit hard so a stuck reader/writer thread can never keep us alive.
///
/// `std::process::exit` runs the runtime's exit path (stdout cleanup, then
/// `ExitProcess`, which tears down the other threads and runs DLL detach
/// under the loader lock). With a thread still blocked in a synchronous
/// `ReadFile` on the agent's stdin pipe that path is not guaranteed to be
/// quick on a loaded machine. `{}` was already written and flushed, so end
/// the process at once instead.
fn exit_now() -> ! {
    let _ = std::io::stdout().flush();
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        // SAFETY: terminating our own process with exit code 0.
        unsafe {
            TerminateProcess(GetCurrentProcess(), 0);
        }
    }
    std::process::exit(0);
}

/// GUI-subsystem exe started from a console without redirection: write to the
/// parent's console (best effort; the shell may print its prompt first).
#[cfg(windows)]
fn use_parent_console() {
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
fn use_parent_console() {}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn relay(args: &[String], start: Instant) {
    let (Some(terminal_id), Some(socket)) =
        (env_nonempty(ENV_TERMINAL_ID), env_nonempty(ENV_SOCKET))
    else {
        return;
    };
    let Some(agent) = args.first().and_then(|a| HookAgent::parse(a)) else {
        return;
    };
    let event = args.get(1).cloned().unwrap_or_default();

    let payload = read_stdin(STDIN_BUDGET);
    let mut msg = HookMessage::new(terminal_id, agent, event, payload);
    msg.token = env_nonempty(ENV_HOOK_TOKEN);
    msg.sent_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // podd drops a line over its limit, and the event with it: cut the
    // payload, never the event (daemon-05).
    let mut line = msg.to_wire_line(MAX_HOOK_LINE);
    line.push('\n');

    let remaining = DEADLINE.saturating_sub(start.elapsed());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(send_line(
            &socket,
            line.as_bytes(),
            Instant::now() + remaining,
        ));
    });
    let _ = rx.recv_timeout(remaining);
}

/// Read stdin with a time budget (the agent might not close it).
fn read_stdin(budget: Duration) -> Value {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let r = std::io::stdin()
            .lock()
            .take(STDIN_LIMIT + 1)
            .read_to_end(&mut buf);
        let _ = tx.send(r.map(|_| buf));
    });
    match rx.recv_timeout(budget) {
        Ok(Ok(buf)) if (buf.len() as u64) <= STDIN_LIMIT => {
            serde_json::from_slice(&buf).unwrap_or(Value::Null)
        }
        Ok(Ok(_)) => serde_json::json!({ TRUNCATED_KEY: { "payload_bytes_over": STDIN_LIMIT } }),
        _ => Value::Null,
    }
}

#[cfg(windows)]
fn send_line(pipe: &str, line: &[u8], deadline: Instant) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
    const ERROR_PIPE_BUSY: i32 = 231;
    loop {
        // Whoever serves this pipe name (podd, or a process that took the
        // name while podd was down) may identify us, never impersonate us.
        match std::fs::OpenOptions::new()
            .write(true)
            .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
            .open(pipe)
        {
            Ok(mut f) => {
                f.write_all(line)?;
                return f.flush();
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(unix)]
fn send_line(path: &str, line: &[u8], deadline: Instant) -> std::io::Result<()> {
    let mut s = std::os::unix::net::UnixStream::connect(path)?;
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1));
    s.set_write_timeout(Some(left))?;
    s.write_all(line)
}

// ─────────────────────────── install --dry-run ───────────────────────────

fn install_cli(args: &[String]) -> i32 {
    let mut dry_run = false;
    let mut uninstall = false;
    let mut block_only = false;
    let mut agents = vec![HookAgent::Claude, HookAgent::Codex];
    let mut home: Option<PathBuf> = None;
    let mut claude_dir: Option<PathBuf> = None;
    let mut codex_home: Option<PathBuf> = None;
    let mut exe: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| -> Option<String> {
            let v = it.next().cloned();
            if v.is_none() {
                eprintln!("pod-hook install: {name} needs a value");
            }
            v
        };
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--uninstall" => uninstall = true,
            "--block-only" => block_only = true,
            "--agent" => match val("--agent").as_deref() {
                Some("all") => agents = vec![HookAgent::Claude, HookAgent::Codex],
                Some(x) => match HookAgent::parse(x) {
                    Some(ag) => agents = vec![ag],
                    None => {
                        eprintln!("pod-hook install: unknown agent {x}");
                        return 2;
                    }
                },
                None => return 2,
            },
            "--home" => home = val("--home").map(PathBuf::from),
            "--claude-config-dir" => claude_dir = val("--claude-config-dir").map(PathBuf::from),
            "--codex-home" => codex_home = val("--codex-home").map(PathBuf::from),
            "--exe" => exe = val("--exe").map(PathBuf::from),
            other => {
                eprintln!("pod-hook install: unknown option {other}");
                return 2;
            }
        }
    }
    if !dry_run {
        eprintln!(
            "pod-hook install: only --dry-run is supported; Pod never writes agent configuration from this CLI"
        );
        return 2;
    }
    let exe = exe
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("pod-hook.exe"));
    let spec = HookCommandSpec::new(exe);

    if block_only {
        let mut out = serde_json::Map::new();
        for ag in &agents {
            out.insert(ag.as_str().into(), install::hooks_block(*ag, &spec));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default()
        );
        return 0;
    }

    let scope = match (claude_dir, codex_home) {
        (None, None) => match home.or_else(install::user_home) {
            Some(home) => InstallScope::Global { home },
            None => {
                eprintln!("pod-hook install: no home directory; pass --home");
                return 2;
            }
        },
        (Some(c), Some(x)) => InstallScope::Profile {
            claude_config_dir: c,
            codex_home: x,
        },
        _ => {
            eprintln!("pod-hook install: --claude-config-dir and --codex-home go together");
            return 2;
        }
    };
    let plan = if uninstall {
        install::plan_uninstall(&scope, &spec, &agents)
    } else {
        install::plan_install(&scope, &spec, &agents)
    };
    match plan {
        Ok(p) => {
            println!("{}", serde_json::to_string_pretty(&p).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("pod-hook install: {e}");
            1
        }
    }
}
