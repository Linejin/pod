//! `pod-hook` — relay an agent hook event to podd.
//!
//! ```text
//! pod-hook claude|codex <Event>     # invoked by the agent; hook JSON on stdin
//! pod-hook install --dry-run [...]  # print the hooks config Pod would install
//! pod-hook statusline [--passthrough [--ascii]] # Claude Code statusLine: keep the usage limits
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
//!
//! Status-line mode (`statusline`) is not a hook: it answers with a status
//! line (or the input unchanged) instead of `{}`, and stores only Claude
//! Code's documented usage-limit numbers for Pod's usage monitor
//! (`statusline.rs`). Same rules otherwise: bounded, no window, no child.
//!
//! Layout: `main` dispatches on the first argument and owns the `{}` answer
//! and the hard exit; `relay` is hook mode, `install` the dry-run planner,
//! `statusline` the usage collector, `console` the parent-console attach for
//! the CLI paths.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod console;
mod install;
mod relay;
mod statusline;

use std::io::Write;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        args.first().map(String::as_str),
        Some("install" | "-h" | "--help" | "help" | "--version" | "-V")
    ) {
        console::use_parent_console();
    }
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        // `pod hooks status` compares the installed copy with this build.
        println!("pod-hook {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.first().map(String::as_str) == Some("install") {
        std::process::exit(install::run(&args[1..]));
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
    if args.first().map(String::as_str) == Some("statusline") {
        let code = std::panic::catch_unwind(|| statusline::run(&args[1..], start)).unwrap_or(0);
        if code != 0 {
            std::process::exit(code);
        }
        exit_now();
    }
    // Print first: the agent gets its answer no matter what happens next.
    {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(b"{}\n");
        let _ = out.flush();
    }
    let _ = std::panic::catch_unwind(|| relay::run(&args, start));
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
