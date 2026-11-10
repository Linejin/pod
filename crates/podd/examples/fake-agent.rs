//! Test-only stand-in for an agent CLI: runs a script of steps given as
//! arguments, writing OSC 7501 reports, window titles and text to the
//! terminal. Copy it as `codex.exe` / `claude.exe` to look like an agent to
//! the process heuristics.
//!
//! ```text
//! fake-agent osc:state=working read osc:state=done hold
//! ```
//! Steps:
//! - `osc:<payload>`   `ESC ] 7501 ; <payload> BEL`
//! - `title:<text>`    `ESC ] 0 ; <text> BEL`
//! - `print:<text>`    text + CRLF
//! - `sleep:<ms>`
//! - `read`            wait for one line on stdin (the test drives the pace)
//! - `hook:<agent>:<event>:<json>`  run `$POD_HOOK_EXE <agent> <event>` with
//!   `<json>` on stdin, like a real agent hook (inherits this terminal's env)
//! - `wait-file:<path>`  wait until the file exists (the test drives the pace
//!   without typing into the terminal)
//! - `paste-loop:<log>`  like an agent TUI waiting for prompts: raw VT input,
//!   bracketed paste on (`ESC[?2004h`), reports `idle`; every submission
//!   (paste and/or typed text, then Enter) is printed, appended to `<log>`
//!   followed by a `<<<END pastes=<n>>>>` line (bracketed pastes seen so
//!   far), and answered with a short
//!   `working` -> `idle` turn. Never returns.
//! - `paste-loop-silent:<log>`  the same, but never reports a state (no OSC
//!   7501): like a shell or an agent known only through heuristics.
//! - `burst-loop:<log>[:<us per key>]`  like Codex on Windows (crossterm):
//!   reads console *key events* (no VT input mode, so the console host
//!   translates what podd types), turns on bracketed paste like crossterm,
//!   and applies Codex's paste-burst rules: characters arriving within 8 ms
//!   of each other are a burst; the burst is shown only after 60 ms without
//!   characters; Enter while a burst is active or within 120 ms of its last
//!   character inserts a newline instead of submitting. `<us per key>`
//!   (default 0) is the processing cost per key event, i.e. how far the
//!   program lags behind its input. Submissions are logged like
//!   `paste-loop` (`<<<END keys=<n>>>>`) and answered with a `working` ->
//!   `idle` turn (OSC 7501). Like Codex, nothing is drawn for typed or
//!   pasted characters until they settle (8 ms typed, 60 ms burst), a
//!   submission drops trailing whitespace, and Enter on an empty input box
//!   does nothing. Both loops log a submission before reporting `working`.
//! - `flood:<n>`        n numbered lines (`line <i> .....`) in one write
//! - `swallow-enters:<n>`  (before `burst-loop`) the first `<n>` Enters that
//!   would submit insert a newline instead: an agent that missed them.
//! - `orch-loop:<base>`  an orchestration agent TUI (docs/19 tests): like
//!   `paste-loop` (submissions logged to `<dir>/submits.log`), where `<dir>`
//!   is `<base>/<POD_ATTEMPT_ID>` (or `<base>/coord`). It also watches
//!   `<dir>`: `osc` (an OSC 7501 payload to report, then deleted) and
//!   `cmd.json` (`[["orch","ask",...], ...]`: runs `pod` from
//!   `<base>/pod-exe.txt` with each argv in one `working` turn, inheriting
//!   this terminal's env, writing `<dir>/out-<n>.json` =
//!   `{args, code, stdout, stderr}`, then reports `idle`). An argv starting
//!   with `!git` runs git instead, `["!write", file, text]` writes a file
//!   (both in the agent's cwd). Never returns.
//! - `hold`           sleep forever (until killed)
//! - `exit:<code>`

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// `paste-loop-silent`: no OSC 7501 reports.
static SILENT: AtomicBool = AtomicBool::new(false);
/// `orch-loop` mode.
static ORCH: AtomicBool = AtomicBool::new(false);
/// `swallow-enters:<n>`.
static SWALLOW: AtomicUsize = AtomicUsize::new(0);

/// An OSC 7501 report, or nothing in silent mode.
fn osc(state: &str) -> String {
    if SILENT.load(Ordering::Relaxed) {
        String::new()
    } else {
        format!("\x1b]7501;state={state}\x07")
    }
}

/// Like real agent TUIs: turn on VT processing, otherwise the console host
/// treats ESC as text (the bundled OpenConsole then drops OSC 7501).
fn enable_vt() {
    use windows_sys::Win32::System::Console::{
        ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    // SAFETY: console mode calls on our own stdout handle.
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(
                h,
                mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            );
        }
    }
}

fn main() {
    enable_vt();
    let mut out = std::io::stdout();
    for step in std::env::args().skip(1) {
        let (op, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        match op {
            "osc" => {
                let _ = write!(out, "\x1b]7501;{arg}\x07");
            }
            "title" => {
                let _ = write!(out, "\x1b]0;{arg}\x07");
            }
            "print" => {
                let _ = write!(out, "{arg}\r\n");
            }
            "sleep" => {
                let _ = out.flush();
                std::thread::sleep(Duration::from_millis(arg.parse().unwrap_or(0)));
            }
            "read" => {
                let _ = out.flush();
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
            }
            "hook" => {
                let mut parts = arg.splitn(3, ':');
                let agent = parts.next().unwrap_or("claude");
                let event = parts.next().unwrap_or("Stop");
                let json = parts.next().unwrap_or("{}");
                let exe = std::env::var("POD_HOOK_EXE").unwrap_or_else(|_| "pod-hook".into());
                let child = Command::new(exe)
                    .args([agent, event])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn();
                match child {
                    Ok(mut c) => {
                        if let Some(mut i) = c.stdin.take() {
                            let _ = i.write_all(json.as_bytes());
                        }
                        let o = c.wait_with_output();
                        let reply = o
                            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                            .unwrap_or_default();
                        let _ = write!(out, "hook {event} -> {reply}\r\n");
                    }
                    Err(e) => {
                        let _ = write!(out, "hook spawn failed: {e}\r\n");
                    }
                }
            }
            "wait-file" => {
                let _ = out.flush();
                while !std::path::Path::new(arg).exists() {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            "paste-loop" => paste_loop(&mut out, arg),
            "orch-loop" => orch_loop(&mut out, arg),
            "paste-loop-silent" => {
                SILENT.store(true, Ordering::Relaxed);
                paste_loop(&mut out, arg)
            }
            "flood" => {
                // `flood:<n>`: n numbered lines in one write (bulk output).
                let n: usize = arg.parse().unwrap_or(1000);
                let text: String = (1..=n)
                    .map(|i| format!("line {i} ......................................\r\n"))
                    .collect();
                let _ = out.write_all(text.as_bytes());
            }
            "swallow-enters" => SWALLOW.store(arg.parse().unwrap_or(1), Ordering::Relaxed),
            "burst-loop" => {
                let (log, cost) = match arg.rsplit_once(':') {
                    Some((l, c)) if c.parse::<u64>().is_ok() && l.len() > 1 => {
                        (l, c.parse().unwrap_or(0))
                    }
                    _ => (arg, 0),
                };
                burst_loop(&mut out, log, cost)
            }
            "hold" => loop {
                let _ = out.flush();
                std::thread::sleep(Duration::from_secs(3600));
            },
            "exit" => {
                let _ = out.flush();
                std::process::exit(arg.parse().unwrap_or(0));
            }
            other => {
                let _ = write!(out, "fake-agent: unknown step {other}\r\n");
            }
        }
        let _ = out.flush();
    }
}

fn raw_vt_input() {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_INPUT, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };
    // SAFETY: console mode call on our own stdin handle.
    unsafe {
        SetConsoleMode(
            GetStdHandle(STD_INPUT_HANDLE),
            ENABLE_VIRTUAL_TERMINAL_INPUT,
        );
    }
}

fn paste_loop(out: &mut std::io::Stdout, log: &str) -> ! {
    use std::io::Read;
    raw_vt_input();
    let _ = write!(out, "\x1b[?2004h{}ready for prompts\r\n", osc("idle"));
    let _ = out.flush();
    const START: &[u8] = b"\x1b[200~";
    const END: &[u8] = b"\x1b[201~";
    let mut pending: Vec<u8> = Vec::new(); // unparsed input
    let mut msg: Vec<u8> = Vec::new(); // current submission
    let mut in_paste = false;
    let mut pastes = 0usize;
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => std::thread::sleep(Duration::from_millis(20)),
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                parse(
                    &mut pending,
                    &mut msg,
                    &mut in_paste,
                    &mut pastes,
                    (START, END),
                    out,
                    log,
                );
            }
        }
    }
}

fn orch_loop(out: &mut std::io::Stdout, base: &str) -> ! {
    use std::io::Read;
    use std::path::PathBuf;
    let me = std::env::var("POD_ATTEMPT_ID").unwrap_or_else(|_| "coord".into());
    ORCH.store(true, Ordering::Relaxed);
    let dir = PathBuf::from(base).join(&me);
    let _ = std::fs::create_dir_all(&dir);
    raw_vt_input();
    let _ = write!(out, "\x1b[?2004h{}orch agent {me} ready\r\n", osc("idle"));
    let _ = out.flush();
    let log = dir.join("submits.log").display().to_string();
    std::thread::spawn(move || {
        let mut out = std::io::stdout();
        let (mut pending, mut msg, mut in_paste, mut pastes) =
            (Vec::new(), Vec::new(), false, 0usize);
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => std::thread::sleep(Duration::from_millis(20)),
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    parse(
                        &mut pending,
                        &mut msg,
                        &mut in_paste,
                        &mut pastes,
                        (b"\x1b[200~", b"\x1b[201~"),
                        &mut out,
                        &log,
                    );
                }
            }
        }
    });
    let pod = std::fs::read_to_string(PathBuf::from(base).join("pod-exe.txt"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "pod".into());
    let mut n = 0usize;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let oscf = dir.join("osc");
        if let Ok(p) = std::fs::read_to_string(&oscf) {
            let _ = std::fs::remove_file(&oscf);
            let _ = write!(out, "\x1b]7501;{}\x07osc {}\r\n", p.trim(), p.trim());
            let _ = out.flush();
        }
        let cmdf = dir.join("cmd.json");
        let Ok(text) = std::fs::read_to_string(&cmdf) else {
            continue;
        };
        let _ = std::fs::remove_file(&cmdf);
        let cmds: Vec<Vec<String>> = serde_json::from_str(&text).unwrap_or_default();
        let _ = write!(
            out,
            "{}running {} command(s)\r\n",
            osc("working"),
            cmds.len()
        );
        let _ = out.flush();
        for args in cmds {
            n += 1;
            // `!git ...` runs git and `!write <file> <text>` writes a file,
            // both in the agent's working directory (its worktree): fake
            // work for the dogfood scenario (tools/e2e/orch-dogfood.ps1).
            let run = |c: &mut Command| match c.stdin(Stdio::null()).output() {
                Ok(o) => (
                    o.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&o.stdout).into_owned(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                ),
                Err(e) => (-1, String::new(), e.to_string()),
            };
            let (code, so, se) = match args.first().map(String::as_str) {
                Some("!git") => run(Command::new("git").args(&args[1..])),
                Some("!write") if args.len() == 3 => match std::fs::write(&args[1], &args[2]) {
                    Ok(()) => (0, String::new(), String::new()),
                    Err(e) => (1, String::new(), e.to_string()),
                },
                _ => run(Command::new(&pod).args(&args)),
            };
            let _ = write!(out, "pod {} -> {code}\r\n", args.join(" "));
            let _ = out.flush();
            let v = serde_json::json!({"args": args, "code": code, "stdout": so, "stderr": se});
            let tmp = dir.join(format!("out-{n}.tmp"));
            let _ = std::fs::write(&tmp, v.to_string());
            let _ = std::fs::rename(&tmp, dir.join(format!("out-{n}.json")));
        }
        let _ = write!(out, "{}turn over\r\n", osc("idle"));
        let _ = out.flush();
    }
}

/// Longest suffix of `buf` that is a proper prefix of `marker`.
fn partial_marker(buf: &[u8], marker: &[u8]) -> usize {
    (1..marker.len().min(buf.len() + 1))
        .rev()
        .find(|&k| buf.ends_with(&marker[..k]))
        .unwrap_or(0)
}

fn parse(
    pending: &mut Vec<u8>,
    msg: &mut Vec<u8>,
    in_paste: &mut bool,
    pastes: &mut usize,
    (start, end): (&[u8], &[u8]),
    out: &mut std::io::Stdout,
    log: &str,
) {
    loop {
        let marker = if *in_paste { end } else { start };
        if let Some(i) = pending.windows(marker.len()).position(|w| w == marker) {
            // Text before the marker.
            let before: Vec<u8> = pending.drain(..i).collect();
            pending.drain(..marker.len());
            if *in_paste {
                msg.extend(before);
            } else {
                typed(&before, msg, *pastes, out, log);
                *pastes += 1;
            }
            *in_paste = !*in_paste;
            continue;
        }
        let keep = partial_marker(pending, marker);
        let chunk: Vec<u8> = pending.drain(..pending.len() - keep).collect();
        if *in_paste {
            msg.extend(chunk);
        } else {
            typed(&chunk, msg, *pastes, out, log);
        }
        return;
    }
}

/// Bytes typed outside a paste: Enter submits.
fn typed(bytes: &[u8], msg: &mut Vec<u8>, pastes: usize, out: &mut std::io::Stdout, log: &str) {
    for &b in bytes {
        if b == b'\r' {
            submit(msg, pastes, out, log);
        } else if b != b'\n' {
            msg.push(b);
        }
    }
}

fn submit(msg: &mut Vec<u8>, pastes: usize, out: &mut std::io::Stdout, log: &str) {
    let text = String::from_utf8_lossy(msg).replace('\r', "\n");
    msg.clear();
    let lines: Vec<&str> = text.split('\n').collect();
    record(log, &format!("{text}\n<<<END pastes={pastes}>>>\n"));
    if ORCH.load(Ordering::Relaxed) && text.starts_with("Read the task brief") {
        // `orch-loop`: the brief's first line is noted, no turn (the test
        // drives the worker's turns through cmd.json).
        let _ = write!(out, "brief noted\r\n");
        let _ = out.flush();
        return;
    }
    let _ = write!(
        out,
        "{}RECEIVED {} line(s) (pastes so far {})\r\n",
        osc("working"),
        lines.len(),
        pastes
    );
    for l in &lines {
        let _ = write!(out, "> {l}\r\n");
    }
    let _ = out.flush();
    std::thread::sleep(Duration::from_millis(400));
    let _ = write!(out, "{}done\r\n", osc("idle"));
    let _ = out.flush();
}

/// Paste-burst thresholds of Codex's TUI (`paste_burst.rs`, Windows values).
const BURST_CHAR_INTERVAL: Duration = Duration::from_millis(8);
const BURST_IDLE_FLUSH: Duration = Duration::from_millis(60);
const BURST_ENTER_SUPPRESS: Duration = Duration::from_millis(120);

/// Console input as crossterm reads it on Windows: key events, raw mode.
fn burst_loop(out: &mut std::io::Stdout, log: &str, cost_us: u64) -> ! {
    use std::time::Instant;
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, GetConsoleMode, GetStdHandle, INPUT_RECORD, KEY_EVENT,
        ReadConsoleInputW, STD_INPUT_HANDLE, SetConsoleMode,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    // SAFETY: console calls on our own stdin handle with valid buffers.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    unsafe {
        let mut mode = 0;
        GetConsoleMode(input, &mut mode);
        // crossterm raw mode: no line input, echo or processed input.
        SetConsoleMode(input, ENABLE_EXTENDED_FLAGS);
    }
    let _ = write!(out, "\x1b[?2004h{}ready for prompts\r\n", osc("idle"));
    let _ = out.flush();

    let mut composer: Vec<u16> = Vec::new();
    let mut keys = 0usize;
    let mut last_char: Option<Instant> = None;
    let mut active = false; // burst being buffered (not shown yet)
    let mut unshown = false; // input not drawn yet
    let mut window_until: Option<Instant> = None;
    let mut records: Vec<INPUT_RECORD> = Vec::with_capacity(256);
    loop {
        // Draw settled input: a typed key after 8 ms, a burst after 60 ms.
        let now = Instant::now();
        let limit = if active {
            BURST_IDLE_FLUSH
        } else {
            BURST_CHAR_INTERVAL
        };
        if unshown && last_char.is_some_and(|t| now.duration_since(t) > limit) {
            active = false;
            unshown = false;
            let _ = write!(out, "[composer: {} chars]\r\n", composer.len());
            let _ = out.flush();
        }
        // SAFETY: waiting on our stdin handle.
        if unsafe { WaitForSingleObject(input, 5) } != 0 {
            continue;
        }
        let mut n = 0u32;
        // SAFETY: `records` has capacity for 256 entries; `n` receives the count.
        let ok = unsafe { ReadConsoleInputW(input, records.as_mut_ptr(), 256, &mut n) };
        if ok == 0 {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        // SAFETY: ReadConsoleInputW initialised `n` records.
        unsafe { records.set_len(n as usize) };
        for r in &records {
            if r.EventType != KEY_EVENT as u16 {
                continue;
            }
            // SAFETY: EventType says this is a key event.
            let k = unsafe { r.Event.KeyEvent };
            let ch = unsafe { k.uChar.UnicodeChar };
            if k.bKeyDown == 0 || ch == 0 {
                continue;
            }
            for _ in 0..k.wRepeatCount.max(1) {
                keys += 1;
                if cost_us > 0 {
                    let until = Instant::now() + Duration::from_micros(cost_us);
                    while Instant::now() < until {
                        std::hint::spin_loop();
                    }
                }
                let now = Instant::now();
                if ch == u16::from(b'\r') {
                    let in_window = window_until.is_some_and(|w| now <= w);
                    let left = SWALLOW.load(Ordering::Relaxed);
                    let swallow = !(active || in_window) && left > 0;
                    if swallow {
                        SWALLOW.store(left - 1, Ordering::Relaxed);
                    }
                    if active || in_window || swallow {
                        unshown = true;
                        composer.push(u16::from(b'\n'));
                        if active {
                            window_until = Some(now + BURST_ENTER_SUPPRESS);
                        }
                    } else if composer_text(&composer).is_empty() {
                        // Like Codex: Enter on an empty input box does nothing.
                        window_until = None;
                    } else {
                        let _ = write!(out, "\r\n");
                        burst_submit(&mut composer, keys, out, log);
                        keys = 0;
                        window_until = None;
                    }
                    continue;
                }
                let fast = last_char.is_some_and(|t| now.duration_since(t) <= BURST_CHAR_INTERVAL);
                if fast {
                    active = true;
                }
                last_char = Some(now);
                composer.push(ch);
                unshown = true;
                if active {
                    window_until = Some(now + BURST_ENTER_SUPPRESS);
                }
            }
        }
        records.clear();
        let _ = out.flush();
    }
}

/// Append a submission to `log` in one write, *before* the agent reports
/// `working`: podd confirms a submission on that report, and a test may read
/// the log as soon as podd answers.
fn record(log: &str, entry: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = f.write_all(entry.as_bytes());
    }
}

/// What a submission sends: like Codex's composer, without trailing
/// whitespace (a newline left by an Enter that landed inside a burst).
fn composer_text(composer: &[u16]) -> String {
    let text = String::from_utf16_lossy(composer).replace('\x1b', "<ESC>");
    text.trim_end().to_string()
}

fn burst_submit(composer: &mut Vec<u16>, keys: usize, out: &mut std::io::Stdout, log: &str) {
    let text = composer_text(composer);
    composer.clear();
    let lines = text.split('\n').count();
    record(log, &format!("{text}\n<<<END keys={keys}>>>\n"));
    let _ = write!(out, "{}RECEIVED {lines} line(s)\r\n", osc("working"));
    let _ = out.flush();
    std::thread::sleep(Duration::from_millis(400));
    let _ = write!(out, "{}done\r\n", osc("idle"));
    let _ = out.flush();
}
